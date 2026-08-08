//! Observations — every recorded disk sighting for a document, by any
//! origin, following the "Three facts, three derivations" model this
//! crate's `sync`/`probe`/`materialize`/`adopt` modules all build on: Ours
//! (journal reconstruction), Theirs (the newest observation, ANY session),
//! Ancestor (`ancestor_at`, THIS session's own agreement history), and
//! `saved_obs` (the CAS expectation, moved only by the four Adoption
//! Contract verbs — `materialize::commit_save`, `adopt::adopt_equal`,
//! `adopt::resolve_adopt`, `adopt::resolve_abandon`).
//!
//! `stat_identity`/`crate::bracket::observe_disk` are the ONE place
//! `vfs.stat` results turn into `Option`-shaped identity facts (D12/D13 —
//! NULL, never a literal 0, when the stat failed or exposed no usable
//! identity). A single successfully-returned read is evidence, never truth
//! (`bracket.rs`'s own module doc) — only a `confirmed: Some(true)`
//! observation may short-circuit a probe, serve as a merge Theirs, or
//! become a CAS baseline; an unconfirmed or unclassified (`None`, legacy)
//! observation decides nothing, though its blob is kept exactly like any
//! other (blob retention is sacred).

use std::path::Path;

use rusqlite::{OptionalExtension, Row, Transaction, params};

use rune_vfs::Vfs;

use crate::Error;
use crate::session::format_rfc3339_nanos;

/// Identifies a row in `observations`. AUTOINCREMENT ids start at 1 — the
/// zero value is never a valid observation.
pub type ObsId = i64;

/// One recorded sighting of a document's disk state. `inode`/`device`/
/// `nlink` are `Option` (D13): a stat failure or unusable identity is a
/// genuine absence, never a literal `0` sharing the column with a real
/// value.
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
    /// The version DAG's first lineage edge, pointing at the prior row this
    /// one succeeds: for an adoption (`materialize::commit_save`,
    /// `adopt::adopt_equal`/`resolve_adopt`), the `saved_obs` baseline the
    /// adoption replaced; for a CONFIRMED fresh disk sighting
    /// (`observe_from_stat_tx`, when `confirmed == Some(true)`) whose hash
    /// differs from what was newest a moment before, that prior newest row.
    /// `None` for a legacy/root row, or a sighting that matched the hash
    /// already newest (nothing to chain to).
    pub parent_a: Option<i64>,
    /// The version DAG's second lineage edge — the disk-side observation a
    /// two-parent join reconciled against: `adopt::resolve_adopt`'s own
    /// `obs` argument (theirs, being resolved), or `materialize`'s swap-race
    /// capture when a save's publish raced a concurrent external write.
    /// `None` for every one-parent row.
    pub parent_b: Option<i64>,
    pub at: String,
    /// `None` for a legacy/unclassified row; `Some(true)` for a sighting
    /// trusted as a stable fact (only such a row may short-circuit a probe,
    /// serve as a merge Theirs, or become a CAS baseline); `Some(false)`
    /// for a sighting that failed classification (an unstable bracket, or a
    /// shrink not yet independently re-sighted) and decides nothing.
    pub confirmed: Option<bool>,
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
/// `blobs.hash`/`observations.blob_hash` both live in.
pub fn hash_bytes(data: &[u8]) -> String {
    crate::blob::hex_sha256(data)
}

const OBS_COLUMNS: &str = "id, doc_id, session_id, blob_hash, seq, size, mtime, inode, device, nlink, origin, parent_a, parent_b, at, confirmed";

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
        parent_a: row.get(11)?,
        parent_b: row.get(12)?,
        at: row.get(13)?,
        confirmed: row.get(14)?,
    })
}

/// The stat facts a single `vfs.stat` call exposes about a path — bundled
/// (rather than threaded as five separate parameters) so every function
/// downstream of a stat call stays under clippy's argument-count lint
/// without resorting to `#[allow(clippy::too_many_arguments)]` (repo rule:
/// no such allow outside test code). `None` identity fields mean the stat
/// failed or exposed no usable identity, never a literal `0` (D12/D13).
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
/// [`StatFacts`] for the same argument-count reason.
#[derive(Clone, Copy, Debug)]
pub struct ObservationMeta<'a> {
    pub blob_hash: &'a str,
    /// The journal position this sighting correlates to; `None` means
    /// uncorrelated (never ancestor-eligible).
    pub seq: Option<i64>,
    /// `'load'|'save'|'watch'|'probe'|'resolve'|'swap'` (schema-enforced).
    pub origin: &'a str,
    /// Carried straight to the `confirmed` column — see [`Observation::confirmed`]
    /// for what each value means.
    pub confirmed: Option<bool>,
}

/// The two lineage edges a row may carry (the `parent_a`/`parent_b`
/// columns), bundled so [`insert_observation_row`] takes the same
/// argument count it always has — see [`StatFacts`]'s doc for why this
/// crate bundles rather than growing a function's raw parameter list.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ParentEdges {
    pub a: Option<ObsId>,
    pub b: Option<ObsId>,
}

/// Inserts a new `observations` row with no lineage edge. Pure SQLite — the
/// caller has already done any disk I/O and blob storage this observation
/// reports on. Every caller that DOES know a predecessor to record instead
/// goes through [`insert_observation_row`] directly.
pub fn record_observation(
    tx: &Transaction<'_>,
    doc_id: i64,
    session_id: i64,
    meta: ObservationMeta<'_>,
    stat: &StatFacts,
    at: &str,
) -> Result<ObsId, Error> {
    insert_observation_row(
        tx,
        doc_id,
        session_id,
        meta,
        stat,
        at,
        ParentEdges::default(),
    )
}

/// The one INSERT every observation row goes through, `parent_a`/`parent_b`
/// included — shared by [`record_observation`] (no edge),
/// [`observe_from_stat_tx`] (the confirmed-sighting edge), and
/// `adopt::record_adoption_tx` (the adoption edges), so the row shape can
/// never drift between the three.
pub(crate) fn insert_observation_row(
    tx: &Transaction<'_>,
    doc_id: i64,
    session_id: i64,
    meta: ObservationMeta<'_>,
    stat: &StatFacts,
    at: &str,
    parents: ParentEdges,
) -> Result<ObsId, Error> {
    tx.execute(
        "INSERT INTO observations(doc_id, session_id, blob_hash, seq, size, mtime, inode, device, nlink, origin, parent_a, parent_b, at, confirmed) \
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
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
            parents.a,
            parents.b,
            at,
            meta.confirmed
        ],
    )?;
    Ok(tx.last_insert_rowid())
}

/// The lineage edge a CONFIRMED fresh sighting records: the doc's own prior
/// newest observation, but only when `new_hash` actually differs from it —
/// a re-confirmation of the same content chains to nothing new. Unconfirmed
/// sightings never chain at all (`confirmed != Some(true)`): a read that
/// isn't trusted as a stable fact earns no lineage claim either.
fn confirmed_sighting_parent(
    tx: &Transaction<'_>,
    doc_id: i64,
    confirmed: Option<bool>,
    new_hash: &str,
) -> Result<Option<ObsId>, Error> {
    if confirmed != Some(true) {
        return Ok(None);
    }
    Ok(newest_observation(tx, doc_id)?
        .filter(|prior| prior.blob_hash != new_hash)
        .map(|prior| prior.id))
}

/// Bundles the disk-sourced payload with the correlation/origin facts that
/// travel together at every "hash it, then record it" call site
/// (`materialize::record_fresh`, `crate::bracket::observe_disk`) —
/// [`observe_from_stat_tx`] puts the blob AND inserts the referencing
/// observation row inside the SAME transaction, so a cross-process blob GC
/// sweep can never land in the gap between the two and starve the insert
/// ([rune-db 2]).
#[derive(Clone, Copy, Debug)]
pub struct ObserveInput<'a> {
    /// Disk-sourced bytes to store as a blob — see `blob.rs`'s module doc
    /// on why this is never gated on UTF-8 validity.
    pub data: &'a [u8],
    /// The journal position this sighting correlates to; `None` means
    /// uncorrelated (never ancestor-eligible).
    pub seq: Option<i64>,
    /// `'load'|'save'|'watch'|'probe'|'resolve'|'swap'` (schema-enforced).
    pub origin: &'a str,
    /// Carried straight to the `confirmed` column — see [`Observation::confirmed`]
    /// for what each value means.
    pub confirmed: Option<bool>,
}

/// The tx-scoped body of [`observe_from_stat`]: puts `input.data` as a blob
/// and records the referencing observation, both against the CALLER's
/// already-open transaction. Exposed `pub(crate)` so a caller that needs
/// MORE than just this observation to commit atomically — `rename::
/// rename_replace`'s swap-capture-and-rebind, which must never let the
/// displaced-bytes observation commit without the document rebind that
/// follows it also committing ([rune-db 4]) — can fold this in alongside
/// its own statements instead of opening a second transaction.
pub(crate) fn observe_from_stat_tx(
    tx: &Transaction<'_>,
    session_id: i64,
    doc_id: i64,
    stat: &StatFacts,
    at: &str,
    input: ObserveInput<'_>,
) -> Result<Observation, Error> {
    let hash = crate::blob::put_blob(tx, input.data)?;
    let parent_a = confirmed_sighting_parent(tx, doc_id, input.confirmed, &hash)?;
    let id = insert_observation_row(
        tx,
        doc_id,
        session_id,
        ObservationMeta {
            blob_hash: &hash,
            seq: input.seq,
            origin: input.origin,
            confirmed: input.confirmed,
        },
        stat,
        at,
        ParentEdges {
            a: parent_a,
            b: None,
        },
    )?;
    Ok(Observation {
        id,
        doc_id,
        session_id,
        blob_hash: hash,
        seq: input.seq,
        size: stat.size,
        mtime: stat.mtime.clone(),
        inode: stat.inode,
        device: stat.device,
        nlink: stat.nlink,
        origin: input.origin.to_string(),
        parent_a,
        parent_b: None,
        at: at.to_string(),
        confirmed: input.confirmed,
    })
}

/// Reads one `observations` row by id. Errors (rather than `Ok(None)`) when
/// missing, since every caller expects `id` to be a real, already-recorded
/// row (a dangling `expect`/`obs` reference IS a bug worth surfacing). A
/// decision-input read — must run inside the deciding `BEGIN IMMEDIATE` tx
/// (plan decision 8), never on the read-only reader connection.
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
/// the "Theirs" derivation (module doc comment). Deliberately session-
/// UNSCOPED (B1): the ONE query in this module that stays that way.
/// Decision-input (plan decision 8).
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
/// `seq <= pos`. `exclude_obs`, when `Some`, names the caller's "theirs"
/// and is excluded from the candidates IFF it is a `load`/`save` row whose
/// correlated seq equals `pos` exactly — the self-reference guard that
/// stops a bare sighting from tautologically counting as its own
/// agreement. The guard deliberately does NOT reach `resolve` rows: a
/// resolve observation at exactly the current journal position is the
/// record of a completed reconciliation (a zero-conflict merge, a Discard,
/// or an all-both resolution journals nothing past it), which is precisely
/// the agreement point a 3-way comparison needs — legitimate as ancestor
/// even while it is also theirs. Narrower still, an OLDER correlation for
/// the same excluded id remains a legitimate ancestor at any origin.
/// Decision-input (plan decision 8).
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
               AND seq IS NOT NULL AND seq <= ?3 AND (id != ?4 OR seq < ?3 OR origin = 'resolve') \
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
/// has). Decision-input — the ONE consumer is `Materialize`'s CAS expect
/// (plan decision 8).
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
    use rusqlite::Connection;
    use std::time::SystemTime;

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
                confirmed: None,
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
                confirmed: None,
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
                confirmed: None,
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
