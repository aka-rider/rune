//! `append_edit` and its coalescing/truncation logic. Split out of
//! `journal.rs` (§1.6) — see that module's doc comment for the shared
//! session-scoping invariant every journal function honors.

use std::time::SystemTime;

use rusqlite::{OptionalExtension, Transaction, params};

use rune_core::buffer::AppliedEdit;
use rune_core::cursor::Cursor;

use crate::Error;
use crate::payload::{cursors_to_json, edits_from_json, edits_to_json};
use crate::session::format_rfc3339_nanos;

/// The coalescing window (`journal.go`, plan Gotchas): a
/// single-rune pure insert is folded into the previous event only when it
/// arrives within this long of it.
const COALESCE_WINDOW: std::time::Duration = std::time::Duration::from_millis(300);

/// Records an edit event in the durable journal for `doc_id`, tagged with
/// `session_id`. A no-op (`Ok(0)`) if `edits` is empty. If this session's
/// `current_seq` is non-NULL (it has undone some of its own events), future
/// events AND snapshots past that position are truncated before inserting
/// the new one, and `current_seq` resets to NULL. Adjacent single-character
/// inserts within 300ms of the previous event are coalesced into it in
/// place — but only when all of `can_coalesce_into`'s guards hold, INCLUDING
/// "no snapshot already anchors that seq" (`journal.go`) — an
/// UPDATE coalesced into an already-snapshotted row would be invisible to a
/// snapshot-anchored `recover_document` replay (CONSTITUTION §1.4.10-adjacent
/// correctness: the row's bytes must never silently outrun what a
/// recovery-anchor reconstruction can see). Returns the journal seq of the
/// inserted (or coalesced) event. Port of `journal.go`.
pub fn append_edit(
    tx: &Transaction<'_>,
    session_id: i64,
    now: SystemTime,
    doc_id: i64,
    edits: &[AppliedEdit],
    cursors_before: &[Cursor],
    cursors_after: &[Cursor],
) -> Result<i64, Error> {
    if edits.is_empty() {
        return Ok(0);
    }

    // Read this session's current undo position; NULL (or no row at all —
    // this session has never journaled anything for doc_id yet) means at
    // head. Port of journal.go.
    let current_seq: Option<i64> = tx
        .query_row(
            "SELECT current_seq FROM session_documents WHERE session_id=?1 AND doc_id=?2",
            params![session_id, doc_id],
            |r| r.get::<_, Option<i64>>(0),
        )
        .optional()?
        .flatten();

    // Truncate an abandoned future — events AND snapshots — scoped to this
    // session's own rows only. Port of journal.go.
    if let Some(cs) = current_seq {
        tx.execute(
            "DELETE FROM events WHERE doc_id=?1 AND session_id=?2 AND seq > ?3",
            params![doc_id, session_id, cs],
        )?;
        // A snapshot anchored past the truncation point describes content
        // that only ever existed in the abandoned future; left alive it
        // becomes a zombie anchor recover_document could still pick,
        // resurrecting truncated bytes under a later edit
        // (journal.go).
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

    // Attempt coalescing with the previous event for this doc, scoped to
    // this session's own rows. Port of journal.go.
    if let Some(only) = as_single_char_insert(edits) {
        let last: Option<(i64, String, String)> = tx
            .query_row(
                "SELECT seq, edits, at FROM events WHERE doc_id=?1 AND session_id=?2 \
                 ORDER BY seq DESC LIMIT 1",
                params![doc_id, session_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;

        if let Some((last_seq, last_edits_json, last_at)) = last {
            let elapsed = elapsed_since(&last_at, now);
            if let Some(elapsed) = elapsed
                && elapsed <= COALESCE_WINDOW
                && can_coalesce_into(&last_edits_json, only)?
            {
                let snapshot_exists: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM snapshots WHERE doc_id=?1 AND seq=?2)",
                    params![doc_id, last_seq],
                    |r| r.get(0),
                )?;

                if !snapshot_exists {
                    let merged_json = merge_edits_json(&last_edits_json, edits)?;
                    let new_after_json = cursors_to_json(cursors_after)?;
                    tx.execute(
                        "UPDATE events SET edits=?1, cursors_after=?2, at=?3 WHERE seq=?4",
                        params![merged_json, new_after_json, now_str, last_seq],
                    )?;
                    return Ok(last_seq);
                }
            }
        }
    }

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
    Ok(new_seq)
}

/// Returns `edits`' single edit if `edits` is a single character insertion
/// (no deletion, a one-rune insert), else `None` — pattern-matched rather
/// than indexed (`edits[0]`) so the single-element access is checked by the
/// match itself, not a runtime bounds check. Port of `journal.go`
/// (`isInsertChar`).
fn as_single_char_insert(edits: &[AppliedEdit]) -> Option<&AppliedEdit> {
    let [only] = edits else { return None };
    (only.deleted.is_empty() && only.insert.chars().count() == 1).then_some(only)
}

/// Reports whether a new single-char insert `next` may coalesce into the
/// previous event's stored `edits_json`: the previous event's LAST edit
/// must itself be a pure insert ending exactly where `next` begins (a
/// genuine typing run — coalescing appends SEQUENTIAL edits to one event,
/// while `apply_inverse`/`reapply` treat an event's edits as a SIMULTANEOUS
/// batch; a typing run is exactly the shape where both readings agree), and
/// must not itself end in whitespace (the word-boundary undo-stop rule).
/// Port of `journal.go` (`canCoalesceInto`).
fn can_coalesce_into(edits_json: &str, next: &AppliedEdit) -> Result<bool, Error> {
    let edits = edits_from_json(edits_json)?;
    let Some(last) = edits.last() else {
        return Ok(false);
    };
    if !last.deleted.is_empty() {
        return Ok(false); // a replace/delete is never part of a typing run
    }
    if next.start != last.start + last.insert.len() {
        return Ok(false); // not adjacent — a fresh undo stop at the new location
    }
    if last
        .insert
        .chars()
        .any(|c| matches!(c, ' ' | '\t' | '\n' | '\r'))
    {
        return Ok(false); // whitespace ends the word — next char starts a new stop
    }
    Ok(true)
}

/// Appends `new_edits` to the edits stored in `existing_json`. Port of
/// `journal.go` (`mergeEditsJSON`).
fn merge_edits_json(existing_json: &str, new_edits: &[AppliedEdit]) -> Result<String, Error> {
    let mut existing = edits_from_json(existing_json)?;
    existing.extend(new_edits.iter().cloned());
    edits_to_json(&existing)
}

/// `now - parse(last_at)`, or `None` if `last_at` doesn't parse (Go's
/// `journal.go`: a parse failure on the previous event's own
/// timestamp silently skips coalescing rather than erroring the whole
/// append — the previous row's `at` was written by this same crate, so a
/// parse failure here only ever indicates a hand-seeded/corrupt test row,
/// never a real production event).
fn elapsed_since(last_at: &str, now: SystemTime) -> Option<std::time::Duration> {
    let last = crate::session::parse_rfc3339_nanos(last_at)?;
    now.duration_since(last).ok()
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

    fn insert_test_document(tx: &Transaction<'_>) -> i64 {
        tx.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('', 'x', 'x')",
            [],
        )
        .expect("insert document");
        tx.last_insert_rowid()
    }

    fn insert_char(pos: usize, ch: &str) -> Vec<AppliedEdit> {
        vec![AppliedEdit {
            start: pos,
            end: pos,
            deleted: String::new(),
            insert: ch.to_string(),
        }]
    }

    /// Port of `TestCoalescingWithinWindow` (`journal_test.go`): two
    /// single-char inserts 200ms apart (well within the 300ms window),
    /// continuing the typing run (each starts where the previous ended),
    /// coalesce into the SAME journal seq — one undo stop covers both.
    #[test]
    fn two_inserts_200ms_apart_coalesce_to_one_seq() {
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

        assert_eq!(seq2, seq1, "200ms-apart typing-run inserts must coalesce");

        let event_count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM events WHERE doc_id=?1",
                params![doc_id],
                |r| r.get(0),
            )
            .expect("count events");
        assert_eq!(event_count, 1, "coalescing must not create a second row");

        let step = undo_peek(&tx, session_id, doc_id)
            .expect("undo_peek")
            .expect("one step to undo");
        assert_eq!(
            step.edits.len(),
            2,
            "one undo stop must cover both coalesced inserts"
        );
        tx.commit().expect("commit");
    }

    /// A snapshot anchored at the seq a coalescing candidate would target
    /// must prevent that coalesce (`journal.go`) — an UPDATE
    /// coalesced in place after the snapshot exists would be invisible to a
    /// snapshot-anchored `recover_document` replay.
    #[test]
    fn snapshot_anchored_at_seq_prevents_coalescing() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("establish session");
        let tx = conn.transaction().expect("tx");
        let doc_id = insert_test_document(&tx);

        let t0 = SystemTime::now();
        let seq1 = append_edit(&tx, session_id, t0, doc_id, &insert_char(0, "a"), &[], &[])
            .expect("append a");
        crate::snapshot::create_snapshot(&tx, session_id, t0, doc_id, "a", seq1)
            .expect("create_snapshot");

        let t1 = t0 + Duration::from_millis(50);
        let seq2 = append_edit(&tx, session_id, t1, doc_id, &insert_char(1, "b"), &[], &[])
            .expect("append b");

        assert_ne!(
            seq2, seq1,
            "coalescing into an already-snapshotted seq must be refused"
        );
        let event_count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM events WHERE doc_id=?1",
                params![doc_id],
                |r| r.get(0),
            )
            .expect("count events");
        assert_eq!(
            event_count, 2,
            "the refused coalesce must land as a new row"
        );
        tx.commit().expect("commit");
    }

    /// Port of `TestCoalescingWhitespaceBreaks`
    /// (`journal_test.go`): a space coalesces into the preceding
    /// non-whitespace stop (same seq), but the event now ENDS in
    /// whitespace, which must break the NEXT coalesce attempt — a fresh
    /// stop.
    #[test]
    fn whitespace_breaks_the_next_coalesce() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("establish session");
        let tx = conn.transaction().expect("tx");
        let doc_id = insert_test_document(&tx);

        let t0 = SystemTime::now();
        let seq_a = append_edit(&tx, session_id, t0, doc_id, &insert_char(0, "a"), &[], &[])
            .expect("append a");

        let t1 = t0 + Duration::from_millis(50);
        let seq_space = append_edit(&tx, session_id, t1, doc_id, &insert_char(1, " "), &[], &[])
            .expect("append space");
        assert_eq!(
            seq_space, seq_a,
            "space must coalesce into the preceding non-whitespace stop"
        );

        let t2 = t1 + Duration::from_millis(50);
        let seq_b = append_edit(&tx, session_id, t2, doc_id, &insert_char(2, "b"), &[], &[])
            .expect("append b");
        assert_ne!(
            seq_b, seq_a,
            "whitespace at the end of the previous event must break the next coalesce"
        );
        tx.commit().expect("commit");
    }

    /// Port of `TestCoalescingOutsideWindow`
    /// (`journal_test.go`): adjacency alone does not coalesce once
    /// the 300ms window has elapsed — a fresh journal stop, two separate
    /// undo steps.
    #[test]
    fn inserts_400ms_apart_do_not_coalesce() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("establish session");
        let tx = conn.transaction().expect("tx");
        let doc_id = insert_test_document(&tx);

        let t0 = SystemTime::now();
        let seq1 = append_edit(&tx, session_id, t0, doc_id, &insert_char(0, "a"), &[], &[])
            .expect("append a");
        let t1 = t0 + Duration::from_millis(400);
        let seq2 = append_edit(&tx, session_id, t1, doc_id, &insert_char(1, "b"), &[], &[])
            .expect("append b");

        assert_eq!(seq2, seq1 + 1, "outside the window must be a fresh stop");
        tx.commit().expect("commit");
    }

    /// Port of `TestTruncateOnNewEdit` (`journal_test.go`): after
    /// undoing past some events, a fresh edit must truncate the abandoned
    /// future so redo is unavailable and undo never resurrects it.
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
            1
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
