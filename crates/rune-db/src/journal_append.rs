//! `append_edit` and its redo-truncation logic, honoring the same
//! session-scoping invariant every journal function shares.

use std::time::SystemTime;

use rusqlite::{OptionalExtension, Transaction, params};

use rune_core::buffer::AppliedEdit;
use rune_core::cursor::Cursor;

use crate::Error;
use crate::ids::{DocId, Seq, SessionId};
use crate::payload::{cursors_to_json, edits_to_json};
use crate::session::format_rfc3339_nanos;

/// Records an edit event in the durable journal for `doc_id`, tagged with
/// `session_id`. A no-op (`Ok(0)`) if `edits` is empty. If this session's
/// `current_seq` is non-NULL (it has undone some of its own events), future
/// events AND snapshots past that position are truncated before inserting
/// the new one, and `current_seq` resets to NULL. Every call inserts a
/// fresh row — one `events` row per local `Journal::push`, always — so the
/// writer seam's own `local_seq` mapping can assert a strict
/// 1:1 correspondence rather than tolerate a coalesced seq reused across
/// two pushes. Returns the journal seq of the inserted event.
pub fn append_edit(
    tx: &Transaction<'_>,
    session_id: SessionId,
    now: SystemTime,
    doc_id: DocId,
    edits: &[AppliedEdit],
    cursors_before: &[Cursor],
    cursors_after: &[Cursor],
) -> Result<Seq, Error> {
    if edits.is_empty() {
        return Ok(Seq(0));
    }

    // Read this session's current undo position; NULL (or no row at all —
    // this session has never journaled anything for doc_id yet) means at
    // head.
    let current_seq: Option<i64> = tx
        .query_row(
            "SELECT current_seq FROM session_documents WHERE session_id=?1 AND doc_id=?2",
            params![session_id, doc_id],
            |r| r.get::<_, Option<i64>>(0),
        )
        .optional()?
        .flatten();

    // Truncate an abandoned future — events AND snapshots — scoped to this
    // session's own rows only.
    if let Some(cs) = current_seq {
        tx.execute(
            "DELETE FROM events WHERE doc_id=?1 AND session_id=?2 AND seq > ?3",
            params![doc_id, session_id, cs],
        )?;
        // A snapshot anchored past the truncation point describes content
        // that only ever existed in the abandoned future; left alive it
        // becomes a zombie anchor recover_document could still pick,
        // resurrecting truncated bytes under a later edit.
        tx.execute(
            "DELETE FROM snapshots WHERE doc_id=?1 AND session_id=?2 AND seq > ?3",
            params![doc_id, session_id, cs],
        )?;
        tx.execute(
            "UPDATE session_documents SET current_seq=NULL WHERE session_id=?1 AND doc_id=?2",
            params![session_id, doc_id],
        )?;
    }

    let now_str = format_rfc3339_nanos(now);
    let edits_json = edits_to_json(edits)?;
    let before_json = cursors_to_json(cursors_before)?;
    let after_json = cursors_to_json(cursors_after)?;

    let new_seq: i64 = tx.query_row(
        "INSERT INTO events(doc_id, session_id, edits, cursors_before, cursors_after, at) \
         VALUES(?1, ?2, ?3, ?4, ?5, ?6) RETURNING seq",
        params![
            doc_id,
            session_id,
            edits_json,
            before_json,
            after_json,
            now_str
        ],
        |r| r.get(0),
    )?;
    Ok(Seq(new_seq))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use std::time::Duration;

    use rusqlite::Connection;

    use super::*;
    use crate::journal::{current_seq, move_undo_pos, redo_peek, undo_peek};

    fn open() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        crate::schema::apply(&conn).expect("schema");
        conn
    }

    fn insert_test_document(tx: &Transaction<'_>) -> DocId {
        tx.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('', 'x', 'x')",
            [],
        )
        .expect("insert document");
        DocId(tx.last_insert_rowid())
    }

    fn insert_char(pos: usize, ch: &str) -> Vec<AppliedEdit> {
        vec![AppliedEdit {
            start: pos,
            end: pos,
            deleted: String::new(),
            insert: ch.to_string(),
        }]
    }

    /// Every `append_edit` call lands its own row, even a run of adjacent
    /// single-character inserts inside what v1's coalescing window used to
    /// fold together — the 1:1 events-row-to-push mapping this deletion
    /// restores.
    #[test]
    fn adjacent_single_char_inserts_never_coalesce() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("establish session");
        let tx = conn.transaction().expect("tx");
        let doc_id = insert_test_document(&tx);

        let t0 = SystemTime::now();
        let seq1 = append_edit(&tx, session_id, t0, doc_id, &insert_char(0, "a"), &[], &[])
            .expect("append a");
        let t1 = t0 + Duration::from_millis(200);
        let seq2 = append_edit(&tx, session_id, t1, doc_id, &insert_char(1, "b"), &[], &[])
            .expect("append b");

        assert_ne!(seq2, seq1, "each insert must land its own row");

        let event_count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM events WHERE doc_id=?1",
                params![doc_id],
                |r| r.get(0),
            )
            .expect("count events");
        assert_eq!(event_count, 2, "no coalescing must ever merge two rows");
        tx.commit().expect("commit");
    }

    /// The regression this deletion exists to fix: three single-char
    /// inserts inside what was the old 300ms coalesce window, then one
    /// undo, must leave `recover_document` matching exactly what the caller
    /// undid to — under v1's coalescing, the writer seam's `local_seq` still
    /// pushed once per insert while the DB folded all three into ONE row,
    /// so undoing one local step reverted the WHOLE merged row and
    /// resurrected the undone character.
    #[test]
    fn three_single_char_inserts_then_one_undo_matches_the_buffer() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("establish session");
        let tx = conn.transaction().expect("tx");
        let doc_id = insert_test_document(&tx);

        let mut t = SystemTime::now();
        append_edit(&tx, session_id, t, doc_id, &insert_char(0, "a"), &[], &[]).expect("append a");
        t += Duration::from_millis(50);
        append_edit(&tx, session_id, t, doc_id, &insert_char(1, "b"), &[], &[]).expect("append b");
        t += Duration::from_millis(50);
        let seq_c = append_edit(&tx, session_id, t, doc_id, &insert_char(2, "c"), &[], &[])
            .expect("append c");

        // One local undo step == one durable event, since every push landed
        // its own row: undoing the single most-recent push must revert
        // exactly "c", never more.
        let step = undo_peek(&tx, session_id, doc_id)
            .expect("undo_peek")
            .expect("something to undo");
        assert_eq!(
            step.new_pos,
            Seq(seq_c.0 - 1),
            "one undo step must land exactly one row back"
        );
        move_undo_pos(&tx, session_id, doc_id, step.new_pos).expect("move_undo_pos");

        let recovered =
            crate::snapshot::recover_document(&tx, session_id, doc_id).expect("recover_document");
        assert_eq!(
            recovered, "ab",
            "undoing the last of three single-char inserts must leave exactly the first two"
        );
        tx.commit().expect("commit");
    }

    /// After undoing past some events, a fresh edit must truncate the
    /// abandoned future so redo is unavailable and undo never resurrects
    /// it.
    #[test]
    fn new_edit_after_undo_truncates_the_abandoned_future() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("establish session");
        let tx = conn.transaction().expect("tx");
        let doc_id = insert_test_document(&tx);

        let mut t = SystemTime::now();
        for ch in ["x", "y", "z"] {
            append_edit(&tx, session_id, t, doc_id, &insert_char(0, ch), &[], &[]).expect("append");
            t += Duration::from_millis(400);
        }

        // Undo twice.
        for _ in 0..2 {
            let step = undo_peek(&tx, session_id, doc_id)
                .expect("undo_peek")
                .expect("something to undo");
            move_undo_pos(&tx, session_id, doc_id, step.new_pos).expect("move_undo_pos");
        }
        assert_eq!(
            current_seq(&tx, session_id, doc_id).expect("current_seq"),
            Seq(1)
        );

        t += Duration::from_millis(400);
        let seq_w = append_edit(&tx, session_id, t, doc_id, &insert_char(0, "w"), &[], &[])
            .expect("append w");
        assert_eq!(
            current_seq(&tx, session_id, doc_id).expect("current_seq"),
            seq_w
        );
        assert!(
            redo_peek(&tx, session_id, doc_id)
                .expect("redo_peek")
                .is_none(),
            "redo must be unavailable after truncate-on-new-edit"
        );
        tx.commit().expect("commit");
    }
}
