//! The Adoption Contract — the only four verbs allowed to move
//! `session_documents.saved_obs` (`observation.go`'s package doc comment,
//! `observation.go:26-39`): `materialize::commit_save` (inlined, since its
//! move must commit in the SAME tx as its re-Bind), [`adopt_equal`],
//! [`resolve_adopt`], and [`resolve_abandon`]. Port of `pkg/docstate/adopt.go`.

use std::time::SystemTime;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::Error;
use crate::observation::{self, ObsId, Observation};
use crate::retry;

/// The shared one-tx BODY behind every path that moves `saved_obs` to a
/// NEWLY-inserted observation: a fresh row is inserted (tagged
/// `session_id`), `supersedes` is set to whatever `session_id`'s
/// `saved_obs` held immediately before (`None` if none), and
/// `session_documents.saved_obs` advances to the new row. Runs entirely
/// inside the CALLER's already-open tx — `materialize::commit_save` calls
/// this directly inside its own save tx (observation + saved_obs + re-Bind
/// must commit atomically together there); [`record_adoption`] below wraps
/// it in its own transaction for standalone callers. Port of
/// `adopt.go:101-150` (`recordAdoptionTx`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_adoption_tx(
    tx: &Transaction<'_>,
    doc_id: i64,
    session_id: i64,
    blob_hash: &str,
    size: i64,
    mtime: &str,
    inode: Option<i64>,
    device: Option<i64>,
    nlink: Option<i64>,
    origin: &str,
    seq: Option<i64>,
    at: &str,
) -> Result<Observation, Error> {
    let supersedes: Option<i64> = tx
        .query_row(
            "SELECT saved_obs FROM session_documents WHERE session_id=?1 AND doc_id=?2",
            params![session_id, doc_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();

    tx.execute(
        "INSERT INTO observations(doc_id, session_id, blob_hash, seq, size, mtime, inode, device, nlink, origin, supersedes, at) \
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![doc_id, session_id, blob_hash, seq, size, mtime, inode, device, nlink, origin, supersedes, at],
    )?;
    let new_id = tx.last_insert_rowid();

    tx.execute(
        "INSERT INTO session_documents(session_id, doc_id, saved_obs) VALUES(?1,?2,?3) \
         ON CONFLICT(session_id, doc_id) DO UPDATE SET saved_obs=excluded.saved_obs",
        params![session_id, doc_id, new_id],
    )?;

    Ok(Observation {
        id: new_id,
        doc_id,
        session_id,
        blob_hash: blob_hash.to_string(),
        seq,
        size,
        mtime: mtime.to_string(),
        inode,
        device,
        nlink,
        origin: origin.to_string(),
        supersedes,
        at: at.to_string(),
    })
}

/// The shared one-tx primitive behind every STANDALONE path that moves
/// `saved_obs` to a newly-inserted observation — [`adopt_equal`],
/// [`resolve_adopt`], and `load::load`'s own first-sighting/heal-adopt
/// cases. Port of `adopt.go:152-177` (`recordAdoption`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_adoption(
    conn: &mut Connection,
    doc_id: i64,
    session_id: i64,
    blob_hash: &str,
    size: i64,
    mtime: &str,
    inode: Option<i64>,
    device: Option<i64>,
    nlink: Option<i64>,
    origin: &str,
    seq: i64,
    now: SystemTime,
) -> Result<Observation, Error> {
    let at = crate::session::format_rfc3339_nanos(now);
    let blob_hash = blob_hash.to_string();
    let mtime = mtime.to_string();
    let origin = origin.to_string();
    retry::with_retry(conn, |tx| {
        record_adoption_tx(
            tx,
            doc_id,
            session_id,
            &blob_hash,
            size,
            &mtime,
            inode,
            device,
            nlink,
            &origin,
            Some(seq),
            &at,
        )
    })
}

/// Promotes a bare sighting (`obs`, already recorded — e.g. `probe::probe`'s
/// bare `'probe'` observation) to a genuine adoption when its content
/// hash-equals the journal-head reconstruction: a NEW `origin='resolve'`
/// observation is inserted, correlated to `head_seq` (making it
/// ancestor-eligible, unlike the bare sighting it promotes), and
/// `saved_obs` advances to it. The crash-between-swap-and-ack recovery
/// path — never used for an ordinary divergence. Port of `adopt.go:179-198`
/// (`AdoptEqual`).
pub fn adopt_equal(
    conn: &mut Connection,
    session_id: i64,
    doc_id: i64,
    obs: ObsId,
    head_seq: i64,
    now: SystemTime,
) -> Result<Observation, Error> {
    let source = retry::with_retry(conn, |tx| observation::get_observation(tx, obs))?;
    record_adoption(
        conn,
        doc_id,
        session_id,
        &source.blob_hash,
        source.size,
        &source.mtime,
        source.inode,
        source.device,
        source.nlink,
        "resolve",
        head_seq,
        now,
    )
}

/// Commits a [D]iscard/[M]erge resolution (or an explicit hash-equality
/// adopt): re-tags `obs` as `origin='resolve'`, correlated to `edit_seq`
/// (the seq of the journaled replace-all/merge-entry edit that resolved
/// it), and advances `saved_obs` to it. Undo past `edit_seq` moves the
/// journal position below this resolve observation, so `ancestor_at`
/// automatically stops finding it and `sync` reports `Diverged` again — the
/// guard re-raises with no bespoke unwind logic. Port of `adopt.go:9-31`
/// (`ResolveAdopt`).
pub fn resolve_adopt(
    conn: &mut Connection,
    session_id: i64,
    doc_id: i64,
    obs: ObsId,
    edit_seq: i64,
    now: SystemTime,
) -> Result<Observation, Error> {
    let source = retry::with_retry(conn, |tx| observation::get_observation(tx, obs))?;
    record_adoption(
        conn,
        doc_id,
        session_id,
        &source.blob_hash,
        source.size,
        &source.mtime,
        source.inode,
        source.device,
        source.nlink,
        "resolve",
        edit_seq,
        now,
    )
}

/// Reverses the resolve observation a merge/discard adoption
/// (`resolve_adopt`) or a heal-adopt created — the Esc-abort-out-of-the-
/// merge-resolver counterpart. Reads `doc_id`'s CURRENT `saved_obs`
/// (expected to be the resolve/adoption observation being unwound), deletes
/// that row (the blob is kept — history is never destroyed, only the fact
/// that it was "agreed" is retracted), and restores `saved_obs` to EXACTLY
/// what it superseded. Refuses (surfaced error) if the current `saved_obs`
/// is not itself an `origin='resolve'` row — abandon unwinds a RESOLUTION
/// and nothing else; deleting a genuine `'save'`/`'load'` baseline would
/// destroy real observation history. A doc with no `saved_obs` at all is a
/// safe no-op. Port of `adopt.go:33-99` (`ResolveAbandon`).
pub fn resolve_abandon(conn: &mut Connection, session_id: i64, doc_id: i64) -> Result<(), Error> {
    retry::with_retry(conn, |tx| {
        let current: Option<i64> = tx
            .query_row(
                "SELECT saved_obs FROM session_documents WHERE session_id=?1 AND doc_id=?2",
                params![session_id, doc_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let Some(current) = current else {
            return Ok(()); // no session_documents row, or saved_obs NULL — nothing adopted yet
        };

        let (supersedes, origin): (Option<i64>, String) = tx.query_row(
            "SELECT supersedes, origin FROM observations WHERE id=?1",
            params![current],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;

        if origin != "resolve" {
            return Err(Error::Invalid(format!(
                "resolve abandon doc {doc_id}: baseline observation {current} has origin {origin:?}, not a resolve adoption — refusing to delete it"
            )));
        }

        // Move saved_obs OFF the row being deleted FIRST — it carries a FK
        // to observations(id), so deleting first would violate that FK.
        tx.execute(
            "UPDATE session_documents SET saved_obs=?1 WHERE session_id=?2 AND doc_id=?3",
            params![supersedes, session_id, doc_id],
        )?;
        tx.execute("DELETE FROM observations WHERE id=?1", params![current])?;
        Ok(())
    })
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

    fn seed_doc(conn: &Connection) -> i64 {
        conn.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        conn.last_insert_rowid()
    }

    /// `observations.blob_hash` is FK-constrained to `blobs.hash` — every
    /// hand-seeded observation in these tests needs a real blob row first.
    fn seed_blob(conn: &Connection, content: &str) -> String {
        crate::blob::put_blob(conn, content).expect("seed blob")
    }

    #[test]
    fn resolve_abandon_restores_exact_prior_baseline() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let doc_id = seed_doc(&conn);
        let hash_1 = seed_blob(&conn, "content 1");

        // First: an ordinary save baseline.
        let first = record_adoption(
            &mut conn,
            doc_id,
            session_id,
            &hash_1,
            1,
            "t",
            None,
            None,
            None,
            "save",
            1,
            SystemTime::now(),
        )
        .expect("first adoption");

        // Then: a resolve adoption on top of it.
        let resolved = adopt_equal(
            &mut conn,
            session_id,
            doc_id,
            first.id,
            2,
            SystemTime::now(),
        )
        .expect("adopt_equal");
        assert_eq!(resolved.supersedes, Some(first.id));

        resolve_abandon(&mut conn, session_id, doc_id).expect("resolve_abandon");

        let current: Option<i64> = conn
            .query_row(
                "SELECT saved_obs FROM session_documents WHERE session_id=?1 AND doc_id=?2",
                params![session_id, doc_id],
                |r| r.get(0),
            )
            .expect("read saved_obs");
        assert_eq!(
            current,
            Some(first.id),
            "must restore the exact prior baseline"
        );

        let deleted: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM observations WHERE id=?1",
                params![resolved.id],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(deleted, 0, "the resolve observation row must be gone");
    }

    #[test]
    fn resolve_abandon_refuses_a_non_resolve_baseline() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let doc_id = seed_doc(&conn);
        let hash_1 = seed_blob(&conn, "content 1");

        record_adoption(
            &mut conn,
            doc_id,
            session_id,
            &hash_1,
            1,
            "t",
            None,
            None,
            None,
            "save",
            1,
            SystemTime::now(),
        )
        .expect("save adoption");

        let err = resolve_abandon(&mut conn, session_id, doc_id).expect_err("must refuse");
        assert!(matches!(err, Error::Invalid(_)));
    }

    #[test]
    fn resolve_abandon_on_a_document_with_no_baseline_is_a_safe_no_op() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let doc_id = seed_doc(&conn);

        resolve_abandon(&mut conn, session_id, doc_id).expect("must be a no-op, not an error");
    }
}
