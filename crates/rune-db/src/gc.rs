//! Startup-and-idle blob GC: sweeps `blobs` rows no longer referenced by any
//! `snapshots` or `observations` row (plan decision 10 — "unreferenced-blob
//! sweep only ... batched ~100 per tx, at startup after the dead-session
//! reaper and on the idle timer. No history retention deletion."). This is
//! entirely distinct from `versioning::gc_old_versions` (WP6.S3), which
//! deletes whole abandoned `rune-v{M}.db` FILES — this module only ever
//! deletes individual `blobs` rows inside the CURRENT database.

use rusqlite::Transaction;

use crate::Error;

/// How many unreferenced blobs a single sweep transaction removes at most —
/// keeps each sweep's `BEGIN IMMEDIATE` short even when a large batch of
/// documents was just deleted (plan WP6.S1: "one blob-sweep batch ... LIMIT
/// 100"). Run repeatedly (idle timer, startup) to work through a bigger
/// backlog over time rather than ever taking one large transaction.
const SWEEP_BATCH_LIMIT: i64 = 100;

/// Deletes up to [`SWEEP_BATCH_LIMIT`] `blobs` rows with zero referencing
/// `snapshots`/`observations` rows, inside `tx`. Returns the number of rows
/// deleted (tests only). Callers wrap this in `retry::with_retry` for the
/// `BEGIN IMMEDIATE` chokepoint (plan Gotchas) — this function issues no
/// transaction control of its own.
pub(crate) fn sweep_unreferenced_blobs(tx: &Transaction) -> Result<usize, Error> {
    let deleted = tx.execute(
        "DELETE FROM blobs WHERE hash IN ( \
            SELECT b.hash FROM blobs b \
            WHERE NOT EXISTS (SELECT 1 FROM snapshots s WHERE s.blob_hash = b.hash) \
              AND NOT EXISTS (SELECT 1 FROM observations o WHERE o.blob_hash = b.hash) \
              AND NOT EXISTS (SELECT 1 FROM merges m WHERE m.marker_hash = b.hash) \
            LIMIT ?1 \
        )",
        rusqlite::params![SWEEP_BATCH_LIMIT],
    )?;
    Ok(deleted)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::test_support::open;
    use rusqlite::{Connection, params};

    fn seed_doc(conn: &Connection) -> i64 {
        conn.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        conn.last_insert_rowid()
    }

    fn seed_session(conn: &Connection) -> i64 {
        conn.execute(
            "INSERT INTO sessions(pid, proc_started_at, opened_at) VALUES(1, 'x', 'x')",
            [],
        )
        .expect("seed session");
        conn.last_insert_rowid()
    }

    /// Port of the WP6.S1 done-when gate verbatim: "create snapshot ->
    /// delete doc -> sweep -> blob gone; blob still referenced by an
    /// observation survives."
    #[test]
    fn sweep_deletes_orphaned_blobs_but_spares_blobs_an_observation_still_references() {
        let mut conn = open();
        let session_id = seed_session(&conn);

        // Blob A: referenced only by a snapshot on a document that then
        // gets deleted (ON DELETE CASCADE takes the snapshot row with it,
        // orphaning the blob it pointed to).
        let doc_a = seed_doc(&conn);
        let hash_a = crate::blob::put_blob(&conn, b"orphan me").expect("put blob a");
        conn.execute(
            "INSERT INTO snapshots(doc_id, session_id, blob_hash, seq, created_at) \
             VALUES (?1, ?2, ?3, 0, 'x')",
            params![doc_a, session_id, hash_a],
        )
        .expect("seed snapshot a");
        conn.execute("DELETE FROM documents WHERE id=?1", params![doc_a])
            .expect("delete doc a (cascades the snapshot away)");

        // Blob B: still referenced by a live observation on a live document.
        let doc_b = seed_doc(&conn);
        let hash_b = crate::blob::put_blob(&conn, b"keep me").expect("put blob b");
        conn.execute(
            "INSERT INTO observations(doc_id, session_id, blob_hash, origin, at) \
             VALUES (?1, ?2, ?3, 'probe', 'x')",
            params![doc_b, session_id, hash_b],
        )
        .expect("seed observation b");

        let tx = conn.transaction().expect("tx");
        let deleted = sweep_unreferenced_blobs(&tx).expect("sweep");
        tx.commit().expect("commit");
        assert_eq!(deleted, 1, "exactly the orphaned blob must be swept");

        let a_gone: bool = conn
            .query_row(
                "SELECT NOT EXISTS(SELECT 1 FROM blobs WHERE hash=?1)",
                params![hash_a],
                |r| r.get(0),
            )
            .expect("check a");
        assert!(a_gone, "the orphaned blob must be gone");

        let b_present: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM blobs WHERE hash=?1)",
                params![hash_b],
                |r| r.get(0),
            )
            .expect("check b");
        assert!(
            b_present,
            "a blob still referenced by an observation must survive"
        );
    }

    #[test]
    fn sweep_spares_a_blob_referenced_only_by_a_merges_row() {
        let mut conn = open();
        let session_id = seed_session(&conn);
        let doc_id = seed_doc(&conn);

        let obs_hash = crate::blob::put_blob(&conn, b"theirs bytes").expect("put obs blob");
        conn.execute(
            "INSERT INTO observations(doc_id, session_id, blob_hash, origin, at) \
             VALUES (?1, ?2, ?3, 'probe', 'x')",
            params![doc_id, session_id, obs_hash],
        )
        .expect("seed observation");
        let theirs_obs = conn.last_insert_rowid();

        let marker_hash = crate::blob::put_blob(&conn, b"<<< markers >>>").expect("put marker");
        conn.execute(
            "INSERT INTO merges(doc_id, session_id, base_obs, theirs_obs, marker_hash, blocks, state, created_at) \
             VALUES (?1, ?2, NULL, ?3, ?4, '[]', 'active', 'x')",
            params![doc_id, session_id, theirs_obs, marker_hash],
        )
        .expect("seed merges row");

        let tx = conn.transaction().expect("tx");
        let deleted = sweep_unreferenced_blobs(&tx).expect("sweep");
        tx.commit().expect("commit");
        assert_eq!(deleted, 0, "nothing here is unreferenced");

        let marker_present: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM blobs WHERE hash=?1)",
                params![marker_hash],
                |r| r.get(0),
            )
            .expect("check marker blob");
        assert!(
            marker_present,
            "a blob referenced only by a merges row must survive the sweep"
        );
    }

    #[test]
    fn sweep_is_a_no_op_when_nothing_is_orphaned() {
        let mut conn = open();
        let tx = conn.transaction().expect("tx");
        let deleted = sweep_unreferenced_blobs(&tx).expect("sweep");
        tx.commit().expect("commit");
        assert_eq!(deleted, 0);
    }
}
