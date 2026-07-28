//! Observations — every recorded disk sighting for a document, by any
//! origin. Ported from Go's observations layer, which documents the "Three
//! facts, three derivations" model this crate's `sync`/`probe`/`materialize`/`adopt`
//! modules all build on: Ours (journal reconstruction), Theirs (the newest
//! observation, ANY session), Ancestor (`ancestor_at`, THIS session's own
//! agreement history), and `saved_obs` (the CAS expectation, moved only by
//! the four Adoption Contract verbs — `materialize::commit_save`,
//! `adopt::adopt_equal`, `adopt::resolve_adopt`, `adopt::resolve_abandon`).
//!
//! `stat_identity`/`observe_from_stat` are the ONE place `vfs.stat` results
//! turn into `Option`-shaped identity facts (D12/D13/§1.7 — NULL, never a
//! literal 0, when the stat failed or exposed no usable identity).
//! `observe_from_stat` itself is NOT a single-tx primitive: the stat (disk
//! I/O) happens first with no transaction open, then a fresh
//! `retry::with_retry` transaction inserts the row — matching the plan's
//! "no DB tx is ever held open across a vfs call" rule for every caller
//! (`probe`/`materialize::record_fresh`) that already did their own
//! disk I/O before calling in.

use std::path::Path;
use std::time::SystemTime;

use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};

use rune_vfs::Vfs;

use crate::Error;
use crate::retry;
use crate::session::format_rfc3339_nanos;

/// Identifies a row in `observations`. AUTOINCREMENT ids start at 1 — the
/// zero value is never a valid observation. Port of `observation.go:79-83`
/// (`ObsID`).
pub type ObsId = i64;

/// One recorded sighting of a document's disk state. Port of
/// `observation.go:85-117` (`Observation`). `inode`/`device`/`nlink` are
/// `Option` (D13/§1.7): a stat failure or unusable identity is a genuine
/// absence, never a literal `0` sharing the column with a real value.
#[derive(Clone, Debug, PartialEq)]
pub struct Observation {
    pub id: ObsId,
    pub doc_id: i64,
    /// WHO recorded this sighting (v10) — required so `ancestor_at`'s
    /// eligibility filter can be scoped to "my own prior agreement";
    /// `newest_observation` ("theirs") deliberately stays unscoped.
    pub session_id: i64,
    pub blob_hash: String,
    /// The journal position this sighting correlates to; `None` means
    /// uncorrelated (never ancestor-eligible) — a bare sighting.
    pub seq: Option<i64>,
    pub size: i64,
    pub mtime: String,
    pub inode: Option<i64>,
    pub device: Option<i64>,
    pub nlink: Option<i64>,
    /// `'load'|'save'|'watch'|'probe'|'resolve'|'swap'` (schema-enforced).
    pub origin: String,
    /// The `saved_obs` this row's adoption replaced, if any.
    pub supersedes: Option<i64>,
    pub at: String,
}

impl Observation {
    /// This observation's own stat facts, repackaged as [`StatFacts`] — for
    /// copy-forward callers (`adopt::adopt_equal`/`resolve_adopt`) that
    /// re-record a PRIOR observation's identity under a new adoption.
    pub fn stat(&self) -> StatFacts {
        StatFacts {
            size: self.size,
            mtime: self.mtime.clone(),
            inode: self.inode,
            device: self.device,
            nlink: self.nlink,
        }
    }
}

/// The lowercase hex SHA-256 of `data` — the same hash space
/// `blobs.hash`/`observations.blob_hash` both live in. Port of
/// `observation.go:207-212` (`hashBytes`).
pub fn hash_bytes(data: &[u8]) -> String {
    crate::blob::hex_sha256(data)
}

const OBS_COLUMNS: &str = "id, doc_id, session_id, blob_hash, seq, size, mtime, inode, device, nlink, origin, supersedes, at";

fn scan_observation(row: &Row<'_>) -> rusqlite::Result<Observation> {
    Ok(Observation {
        id: row.get(0)?,
        doc_id: row.get(1)?,
        session_id: row.get(2)?,
        blob_hash: row.get(3)?,
        seq: row.get(4)?,
        size: row.get(5)?,
        mtime: row.get(6)?,
        inode: row.get(7)?,
        device: row.get(8)?,
        nlink: row.get(9)?,
        origin: row.get(10)?,
        supersedes: row.get(11)?,
        at: row.get(12)?,
    })
}

/// The stat facts a single `vfs.stat` call exposes about a path — bundled
/// (rather than threaded as five separate parameters) so every function
/// downstream of a stat call stays under clippy's argument-count lint
/// without resorting to `#[allow(clippy::too_many_arguments)]` (repo rule:
/// no such allow outside test code). `None` identity fields mean the stat
/// failed or exposed no usable identity, never a literal `0` (D12/D13/§1.7).
/// Port of the facts `observation.go:244-265` (`statIdentity`) returns.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatFacts {
    pub size: i64,
    pub mtime: String,
    pub inode: Option<i64>,
    pub device: Option<i64>,
    pub nlink: Option<i64>,
}

/// Stats `path` through `vfs` and returns the resulting [`StatFacts`]. Pure —
/// no DB access, so callers control exactly when this (disk I/O) runs
/// relative to any open transaction.
pub fn stat_identity(vfs: &dyn Vfs, path: &Path) -> StatFacts {
    match vfs.stat(path) {
        Ok(st) => StatFacts {
            size: st.size as i64,
            mtime: format_rfc3339_nanos(st.mtime),
            inode: st.identity.inode.map(|v| v as i64),
            device: st.identity.device.map(|v| v as i64),
            nlink: st.nlink.map(|v| v as i64),
        },
        Err(_) => StatFacts::default(),
    }
}

/// The non-stat facts describing what an observation records — bundled with
/// [`StatFacts`] for the same argument-count reason. Port of the remaining
/// (non-identity) fields `observation.go:228-242`/`267-290` take.
#[derive(Clone, Copy, Debug)]
pub struct ObservationMeta<'a> {
    pub blob_hash: &'a str,
    /// The journal position this sighting correlates to; `None` means
    /// uncorrelated (never ancestor-eligible).
    pub seq: Option<i64>,
    /// `'load'|'save'|'watch'|'probe'|'resolve'|'swap'` (schema-enforced).
    pub origin: &'a str,
}

/// Inserts a new `observations` row. Pure SQLite — the caller has already
/// done any disk I/O and blob storage this observation reports on. Port of
/// `observation.go:228-242` (`recordObservation`).
pub fn record_observation(
    tx: &Transaction<'_>,
    doc_id: i64,
    session_id: i64,
    meta: ObservationMeta<'_>,
    stat: &StatFacts,
    at: &str,
) -> Result<ObsId, Error> {
    tx.execute(
        "INSERT INTO observations(doc_id, session_id, blob_hash, seq, size, mtime, inode, device, nlink, origin, at) \
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![
            doc_id,
            session_id,
            meta.blob_hash,
            meta.seq,
            stat.size,
            stat.mtime,
            stat.inode,
            stat.device,
            stat.nlink,
            meta.origin,
            at
        ],
    )?;
    Ok(tx.last_insert_rowid())
}

/// Stats `path` and records an observation of `meta.blob_hash` at that
/// metadata. NOT a single transaction: the stat (disk I/O) runs with no tx
/// open, then a fresh `retry::with_retry` inserts the row — the caller
/// (`probe`/`materialize::record_fresh`) is trusted to already have done its
/// own read/blob-store before calling in. Port of `observation.go:267-290`
/// (`observeFromStat`).
pub fn observe_from_stat(
    conn: &mut Connection,
    vfs: &dyn Vfs,
    session_id: i64,
    doc_id: i64,
    path: &Path,
    meta: ObservationMeta<'_>,
    now: SystemTime,
) -> Result<Observation, Error> {
    let stat = stat_identity(vfs, path);
    let at = format_rfc3339_nanos(now);
    let blob_hash = meta.blob_hash.to_string();
    let origin = meta.origin.to_string();

    let id = retry::with_retry(conn, |tx| {
        record_observation(tx, doc_id, session_id, meta, &stat, &at)
    })?;

    Ok(Observation {
        id,
        doc_id,
        session_id,
        blob_hash,
        seq: meta.seq,
        size: stat.size,
        mtime: stat.mtime,
        inode: stat.inode,
        device: stat.device,
        nlink: stat.nlink,
        origin,
        supersedes: None,
        at,
    })
}

/// Reads one `observations` row by id. Errors (rather than `Ok(None)`) when
/// missing — Go's `getObservation` wraps `sql.ErrNoRows` as a genuine error
/// since every caller expects `id` to be a real, already-recorded row (a
/// dangling `expect`/`obs` reference IS a bug worth surfacing). Port of
/// `observation.go:315-328` (`getObservation`). A decision-input read — must
/// run inside the deciding `BEGIN IMMEDIATE` tx (plan decision 8), never on
/// the read-only reader connection.
pub fn get_observation(tx: &Transaction<'_>, id: ObsId) -> Result<Observation, Error> {
    tx.query_row(
        &format!("SELECT {OBS_COLUMNS} FROM observations WHERE id=?1"),
        params![id],
        scan_observation,
    )
    .optional()?
    .ok_or_else(|| Error::NotFound(format!("observation {id}")))
}

/// `doc_id`'s newest recorded observation, by id, ANY origin, ANY session —
/// the "Theirs" derivation (package doc comment). Deliberately session-
/// UNSCOPED (B1): the ONE query in this module that stays that way. Port of
/// `observation.go:330-349` (`newestObservation`). Decision-input (plan
/// decision 8).
pub fn newest_observation(tx: &Transaction<'_>, doc_id: i64) -> Result<Option<Observation>, Error> {
    tx.query_row(
        &format!("SELECT {OBS_COLUMNS} FROM observations WHERE doc_id=?1 ORDER BY id DESC LIMIT 1"),
        params![doc_id],
        scan_observation,
    )
    .optional()
    .map_err(Error::from)
}

/// Derives the 3-way-merge ancestor for `doc_id` AT journal position `pos`,
/// AS SEEN BY `session_id`: the newest observation `session_id` itself
/// recorded, with `origin IN ('load','save','resolve')` and a correlated
/// `seq <= pos`. `exclude_obs`, when `Some`, is excluded from the
/// candidates IFF its own correlated seq equals `pos` exactly (the
/// self-reference guard — narrower than excluding the id outright; an
/// OLDER correlation for the same id is still a legitimate ancestor). Port
/// of `sync.go:7-54` (`ancestorAt`). Decision-input (plan decision 8).
pub fn ancestor_at(
    tx: &Transaction<'_>,
    doc_id: i64,
    session_id: i64,
    pos: i64,
    exclude_obs: Option<ObsId>,
) -> Result<Option<Observation>, Error> {
    let exclude = exclude_obs.unwrap_or(0);
    tx.query_row(
        &format!(
            "SELECT {OBS_COLUMNS} FROM observations \
             WHERE doc_id=?1 AND session_id=?2 AND origin IN ('load','save','resolve') \
               AND seq IS NOT NULL AND seq <= ?3 AND (id != ?4 OR seq < ?3) \
             ORDER BY seq DESC, id DESC LIMIT 1"
        ),
        params![doc_id, session_id, pos, exclude],
        scan_observation,
    )
    .optional()
    .map_err(Error::from)
}

/// `doc_id`'s current CAS expectation AS SEEN BY `session_id`
/// (`session_documents.saved_obs`) — the observation `session_id` believes
/// reflects what is physically on disk right now. `None` means `session_id`
/// has never adopted anything for `doc_id` yet (even if a DIFFERENT session
/// has). Port of `observation.go:351-386` (`SavedObs`/`savedObsFor`).
/// Decision-input — the ONE consumer is `Materialize`'s CAS expect (plan
/// decision 8).
pub fn saved_obs_for(
    tx: &Transaction<'_>,
    session_id: i64,
    doc_id: i64,
) -> Result<Option<Observation>, Error> {
    let obs_id: Option<i64> = tx
        .query_row(
            "SELECT saved_obs FROM session_documents WHERE session_id=?1 AND doc_id=?2",
            params![session_id, doc_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let Some(obs_id) = obs_id else {
        return Ok(None);
    };
    get_observation(tx, obs_id).map(Some)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn open() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        crate::schema::apply(&conn).expect("schema");
        conn
    }

    fn seed_doc(tx: &Transaction<'_>) -> i64 {
        tx.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        tx.last_insert_rowid()
    }

    /// `observations.blob_hash` is FK-constrained to `blobs.hash` — every
    /// hand-seeded observation in these tests needs a real blob row first.
    fn seed_blob(tx: &Transaction<'_>, content: &str) -> String {
        crate::blob::put_blob(tx, content.as_bytes()).expect("seed blob")
    }

    #[test]
    fn get_observation_missing_id_is_an_error_not_a_default() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);
        let _ = session_id;
        let err = get_observation(&tx, 999).expect_err("missing id must error");
        assert!(matches!(err, Error::NotFound(_)));
        let _ = doc_id;
    }

    #[test]
    fn newest_observation_is_unscoped_across_sessions() {
        let mut conn = open();
        let session_a =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session a");
        let session_b =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session b");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);
        let hash_a = seed_blob(&tx, "content a");
        let hash_b = seed_blob(&tx, "content b");

        let stat = StatFacts {
            size: 1,
            mtime: "t".to_string(),
            ..Default::default()
        };
        record_observation(
            &tx,
            doc_id,
            session_a,
            ObservationMeta {
                blob_hash: &hash_a,
                seq: None,
                origin: "probe",
            },
            &stat,
            "t",
        )
        .expect("record a");
        let b_id = record_observation(
            &tx,
            doc_id,
            session_b,
            ObservationMeta {
                blob_hash: &hash_b,
                seq: None,
                origin: "probe",
            },
            &stat,
            "t",
        )
        .expect("record b");

        let newest = newest_observation(&tx, doc_id)
            .expect("newest")
            .expect("some");
        assert_eq!(newest.id, b_id, "newest must be session-unscoped");
        tx.commit().expect("commit");
    }

    #[test]
    fn ancestor_at_self_reference_guard_excludes_only_at_exact_seq() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);
        let hash = seed_blob(&tx, "content");

        let obs_id = record_observation(
            &tx,
            doc_id,
            session_id,
            ObservationMeta {
                blob_hash: &hash,
                seq: Some(5),
                origin: "load",
            },
            &StatFacts {
                size: 1,
                mtime: "t".to_string(),
                ..Default::default()
            },
            "t",
        )
        .expect("record");

        // Excluded at exactly its own correlated seq.
        let at_same_pos = ancestor_at(&tx, doc_id, session_id, 5, Some(obs_id)).expect("query");
        assert!(at_same_pos.is_none(), "a fact cannot be its own ancestor");

        // NOT excluded at a LATER position — still a legitimate ancestor.
        let at_later_pos = ancestor_at(&tx, doc_id, session_id, 6, Some(obs_id)).expect("query");
        assert!(
            at_later_pos.is_some(),
            "an older correlation for the same id is still a legitimate ancestor"
        );
        tx.commit().expect("commit");
    }
}
