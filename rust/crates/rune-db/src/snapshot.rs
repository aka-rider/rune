//! Recovery-anchor snapshots and `recover_document`'s forward replay — port
//! of `pkg/docstate/snapshot.go:74-180`. A snapshot is a PURE recovery
//! anchor: it exists only to bound how far `recover_document` ever has to
//! replay, never a source-of-truth taxonomy (that lives in `observations`,
//! WP4). CONSTITUTION §1.4.10: every snapshot's content flows through
//! `blob::put_blob` before it's ever discarded from the journal's live
//! window (`journal::append_edit`'s future-truncation deletes both `events`
//! and `snapshots` rows, but the blob a surviving snapshot still points to
//! is untouched).

use std::time::SystemTime;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use rune_core::buffer::Buffer;
use rune_core::undo::reapply;

use crate::Error;
use crate::blob::{get_blob, put_blob};
use crate::journal::edits_in_range;
use crate::session::format_rfc3339_nanos;

/// Stores a recovery anchor for `doc_id` at journal position `seq`, tagged
/// with `session_id` — a snapshot anchors ONE session's own replay window;
/// two sessions editing the same `doc_id` keep entirely separate anchor
/// chains. `seq` should be the most recently returned seq from
/// `journal::append_edit` so `recover_document` can find this snapshot as
/// the closest anchor for any replay. Port of `snapshot.go:74-103`.
pub fn create_snapshot(
    tx: &Transaction<'_>,
    session_id: i64,
    now: SystemTime,
    doc_id: i64,
    content: &str,
    seq: i64,
) -> Result<i64, Error> {
    let hash = put_blob(tx, content.as_bytes())?;
    let at = format_rfc3339_nanos(now);
    tx.execute(
        "INSERT INTO snapshots(doc_id, session_id, blob_hash, seq, created_at) \
         VALUES(?1, ?2, ?3, ?4, ?5)",
        params![doc_id, session_id, hash, seq, at],
    )?;
    Ok(tx.last_insert_rowid())
}

/// Reconstructs `doc_id`'s content AS SEEN BY `session_id`:
///
/// 1. Read `session_id`'s own current undo position from
///    `session_documents` (no row, or NULL, means "at head" → use
///    `i64::MAX`).
/// 2. Find the newest snapshot tagged `session_id` with `seq <= target`,
///    tie-broken `seq DESC, id DESC` (coalesced edits keep the SAME seq, so
///    several snapshots can share one seq with progressively newer content
///    — `id DESC` picks the freshest one at that seq, never an arbitrary
///    tie); `anchor_content = ""` if none.
/// 3. Gather `session_id`'s own edit batches with `seq` in
///    `(anchor_seq, target]`.
/// 4. Forward-replay those batches onto `anchor_content`, one row at a
///    time, using `rune_core::undo::reapply`'s edit-application semantics
///    (ascending by `start` within a row, against a running buffer) — NOT
///    Go's `buffer.ReplayForward`, which silently clamps/skips an
///    out-of-range edit instead of erroring (`replay.go:26-33`). Every row
///    here was produced by this crate's own `append_edit`, so a replay
///    failure means a genuinely corrupt/inconsistent journal, which §1.3
///    requires surfacing rather than silently reconstructing wrong content.
///
/// Port of `snapshot.go:105-172` (`recoverAt`/`RecoverDocument`). Takes
/// `&Connection` rather than `&Transaction` — this is a pure read, so
/// `rusqlite`'s `Transaction: Deref<Target=Connection>` lets writer-thread
/// callers (inside `retry::with_retry`'s `&Transaction`) and read-only
/// callers (`reader.rs`, a plain `&Connection`) share this ONE
/// implementation instead of two copies of the same query sequence.
pub fn recover_document(conn: &Connection, session_id: i64, doc_id: i64) -> Result<String, Error> {
    let current_seq: Option<i64> = conn
        .query_row(
            "SELECT current_seq FROM session_documents WHERE session_id=?1 AND doc_id=?2",
            params![session_id, doc_id],
            |r| r.get::<_, Option<i64>>(0),
        )
        .optional()?
        .flatten();
    let target = current_seq.unwrap_or(i64::MAX);

    let anchor: Option<(i64, String)> = conn
        .query_row(
            "SELECT seq, blob_hash FROM snapshots \
             WHERE doc_id=?1 AND session_id=?2 AND seq <= ?3 \
             ORDER BY seq DESC, id DESC LIMIT 1",
            params![doc_id, session_id, target],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;

    let (anchor_seq, anchor_content) = match anchor {
        Some((seq, hash)) => {
            let bytes = get_blob(conn, &hash)?;
            // The anchor blob is always session-authored buffer content
            // (written by `create_snapshot` from a `&str`) re-entering the
            // String-typed edit buffer here — a decode failure means a
            // genuinely corrupt snapshot, which must surface as an error,
            // never be silently coerced (blob.rs module doc).
            let content = String::from_utf8(bytes).map_err(|e| {
                Error::CorruptPayload(format!(
                    "snapshot blob {hash} for doc {doc_id}: non-utf8 content: {e}"
                ))
            })?;
            (seq, content)
        }
        None => (0, String::new()),
    };

    let rows = edits_in_range(conn, session_id, doc_id, anchor_seq, target)?;

    let mut buf = Buffer::new(anchor_content);
    for row in rows {
        buf = reapply(&buf, &row.edits).map_err(|e| {
            Error::ReplayFailed(format!(
                "doc {doc_id} session {session_id} at seq {}: {e}",
                row.seq
            ))
        })?;
    }
    Ok(buf.content().to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::journal::{append_edit, move_undo_pos, undo_peek};
    use rune_core::buffer::AppliedEdit;

    fn open() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        crate::schema::apply(&conn).expect("schema");
        conn.pragma_update(None, "foreign_keys", "ON").expect("fk");
        conn
    }

    fn insert_test_document(tx: &Transaction<'_>) -> i64 {
        tx.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('', 'x', 'x')",
            [],
        )
        .expect("insert document");
        tx.last_insert_rowid()
    }

    fn insert_test_session(conn: &Connection) -> i64 {
        crate::session::establish_session(conn, SystemTime::now()).expect("establish session")
    }

    fn text_insert(s: &str) -> Vec<AppliedEdit> {
        vec![AppliedEdit {
            start: 0,
            end: 0,
            deleted: String::new(),
            insert: s.to_string(),
        }]
    }

    #[test]
    fn recover_with_no_snapshot_replays_from_empty() {
        let mut conn = open();
        let session_id = insert_test_session(&conn);
        let tx = conn.transaction().expect("tx");
        let doc_id = insert_test_document(&tx);

        append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            &text_insert("hello"),
            &[],
            &[],
        )
        .expect("append_edit");

        let got = recover_document(&tx, session_id, doc_id).expect("recover");
        assert_eq!(got, "hello");
        tx.commit().expect("commit");
    }

    #[test]
    fn recover_uses_newest_anchor_at_or_before_target_and_replays_the_rest() {
        let mut conn = open();
        let session_id = insert_test_session(&conn);
        let tx = conn.transaction().expect("tx");
        let doc_id = insert_test_document(&tx);

        append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            &text_insert("file-edit-1"),
            &[],
            &[],
        )
        .expect("append_edit 1");
        let seq1 = crate::journal::current_seq(&tx, session_id, doc_id).expect("seq1");
        create_snapshot(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            "file-edit-1",
            seq1,
        )
        .expect("snapshot");

        append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            &text_insert("-more"),
            &[],
            &[],
        )
        .expect("append_edit 2");

        let got = recover_document(&tx, session_id, doc_id).expect("recover");
        assert_eq!(got, "-morefile-edit-1");
        tx.commit().expect("commit");
    }

    #[test]
    fn recover_respects_current_seq_after_undo() {
        let mut conn = open();
        let session_id = insert_test_session(&conn);
        let tx = conn.transaction().expect("tx");
        let doc_id = insert_test_document(&tx);

        append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            &text_insert("a"),
            &[],
            &[],
        )
        .expect("append a");
        append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            &text_insert("b"),
            &[],
            &[],
        )
        .expect("append b");

        let step = undo_peek(&tx, session_id, doc_id)
            .expect("undo_peek")
            .expect("something to undo");
        move_undo_pos(&tx, session_id, doc_id, step.new_pos).expect("move_undo_pos");

        let got = recover_document(&tx, session_id, doc_id).expect("recover");
        assert_eq!(got, "a", "recovery must stop at the undone position");
        tx.commit().expect("commit");
    }
}
