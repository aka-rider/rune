//! Replay-equivalence property test (plan WP3.S5, the R4 fuzzer-scope
//! resolution: "property tests in WP3-6 are the v1 gate"). Generates random
//! sequences of insert/delete/undo/redo actions and applies each one to TWO
//! independent journals built from the SAME edit batches:
//!
//! (a) an in-memory `rune_core::undo::Journal` driving a `rune_core::buffer::
//!     Buffer` directly (the ground truth every action is computed against);
//! (b) `rune-db`'s `append_edit`/`undo_peek`/`redo_peek`/`move_undo_pos`
//!     against a fresh in-memory SQLite connection, reconstructed via
//!     `recover_document` after every single action.
//!
//! After every action the two must report byte-identical content.
//!
//! Deliberate scope limit: every generated insert is 2+ ASCII characters
//! (`is_insert_char` in `journal.rs` only ever coalesces a SINGLE-rune pure
//! insert), so `append_edit`'s keystroke-coalescing UPDATE never triggers
//! here. Coalescing merges TWO of the ground truth's own `Journal::push`
//! steps into ONE `rune-db` journal row, which would decouple the two
//! journals' undo-stop granularity from each other — an undo action would
//! then legitimately undo a different number of edits on each side. That
//! collapse is real, deliberate DB-only behavior, and it already has its
//! own dedicated deterministic tests (`journal.rs`'s
//! `two_inserts_200ms_apart_coalesce_to_one_seq`,
//! `snapshot_anchored_at_seq_prevents_coalescing`,
//! `whitespace_breaks_the_next_coalesce`) — this property test's job is
//! REPLAY correctness (append/truncate/undo/redo/recover_document), not
//! coalescing, so it sidesteps the granularity mismatch by construction
//! rather than modeling coalescing in the reference journal too.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::time::{Duration, SystemTime};

use proptest::prelude::*;
use rusqlite::Connection;

use rune_core::buffer::{Buffer, Edit};
use rune_core::undo::{Journal, Step as CoreStep, apply_inverse, reapply};
use rune_db::{append_edit, current_seq, move_undo_pos, recover_document, redo_peek, undo_peek};

#[derive(Debug, Clone)]
enum Action {
    Insert { at_frac: u8, text: String },
    Delete { at_frac: u8, len_frac: u8 },
    Undo,
    Redo,
}

fn action_strategy() -> impl Strategy<Value = Action> {
    prop_oneof![
        3 => (any::<u8>(), "[a-zA-Z0-9]{2,6}")
            .prop_map(|(at_frac, text)| Action::Insert { at_frac, text }),
        2 => (any::<u8>(), any::<u8>())
            .prop_map(|(at_frac, len_frac)| Action::Delete { at_frac, len_frac }),
        2 => Just(Action::Undo),
        2 => Just(Action::Redo),
    ]
}

/// Scales `frac` (0..=255) onto a byte offset in `0..=len` — every generated
/// insert/delete offset this way is trivially a valid char boundary because
/// the buffer's content is ASCII-only throughout (inserted text is always
/// `[a-zA-Z0-9]+`), so a byte offset is always a char offset too.
fn frac_to_offset(len: usize, frac: u8) -> usize {
    if len == 0 {
        return 0;
    }
    (usize::from(frac) * len) / 255
}

/// Resolves a delete action against `len`, or `None` if there is nothing to
/// delete (empty buffer) — the delete always removes at least one byte.
fn resolve_delete(len: usize, at_frac: u8, len_frac: u8) -> Option<(usize, usize)> {
    if len == 0 {
        return None;
    }
    let start = frac_to_offset(len, at_frac);
    let remaining = len - start;
    if remaining == 0 {
        return None;
    }
    let del_len = 1 + (usize::from(len_frac) * (remaining - 1)) / 255;
    Some((start, start + del_len))
}

/// A fresh in-memory `rune-db` connection with schema applied, one session,
/// and one document — self-contained setup, no `Store`/writer thread
/// involved (this property test drives the domain functions directly
/// against a single connection for full determinism across hundreds of
/// cases, matching `journal.rs`'s own unit tests).
fn open_test_db() -> (Connection, i64, i64) {
    let conn = Connection::open_in_memory().expect("open in-memory connection");
    conn.execute_batch(rune_db::SCHEMA).expect("apply schema");
    conn.pragma_update(None, "foreign_keys", "ON")
        .expect("enable foreign_keys");

    conn.execute(
        "INSERT INTO sessions(pid, proc_started_at, opened_at) VALUES (1, '', 'x')",
        [],
    )
    .expect("insert session");
    let session_id = conn.last_insert_rowid();

    conn.execute(
        "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('', 'x', 'x')",
        [],
    )
    .expect("insert document");
    let doc_id = conn.last_insert_rowid();

    (conn, session_id, doc_id)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// The replay-equivalence gate itself (plan WP3 Done-when: "the WP3.S5
    /// proptest suite exists ... and passes >= 256 cases").
    #[test]
    fn journal_and_rune_db_reconstruct_identical_content(
        actions in proptest::collection::vec(action_strategy(), 1..40),
    ) {
        let mut buf = Buffer::new("");
        let mut journal = Journal::new();

        let (mut conn, session_id, doc_id) = open_test_db();
        let mut now = SystemTime::now();

        for action in actions {
            now += Duration::from_millis(400); // never inside the coalescing window

            match action {
                Action::Insert { at_frac, text } => {
                    let at = frac_to_offset(buf.len(), at_frac);
                    let edit = Edit {
                        start: at,
                        end: at,
                        insert: text,
                        cursor_id: 0,
                    };
                    let (new_buf, applied) =
                        buf.apply_edits(std::slice::from_ref(&edit)).expect("insert must apply");
                    buf = new_buf;

                    journal.push(CoreStep {
                        edits: applied.clone(),
                        cursors_before: vec![],
                        cursors_after: vec![],
                    });

                    let tx = conn.transaction().expect("begin tx");
                    append_edit(&tx, session_id, now, doc_id, &applied, &[], &[])
                        .expect("db append_edit (insert)");
                    tx.commit().expect("commit");
                }
                Action::Delete { at_frac, len_frac } => {
                    let Some((start, end)) = resolve_delete(buf.len(), at_frac, len_frac) else {
                        continue;
                    };
                    let edit = Edit {
                        start,
                        end,
                        insert: String::new(),
                        cursor_id: 0,
                    };
                    let (new_buf, applied) =
                        buf.apply_edits(std::slice::from_ref(&edit)).expect("delete must apply");
                    buf = new_buf;

                    journal.push(CoreStep {
                        edits: applied.clone(),
                        cursors_before: vec![],
                        cursors_after: vec![],
                    });

                    let tx = conn.transaction().expect("begin tx");
                    append_edit(&tx, session_id, now, doc_id, &applied, &[], &[])
                        .expect("db append_edit (delete)");
                    tx.commit().expect("commit");
                }
                Action::Undo => {
                    let ground = journal.undo_peek().map(|(step, pos)| (step.clone(), pos));

                    let tx = conn.transaction().expect("begin tx");
                    let db_step = undo_peek(&tx, session_id, doc_id).expect("db undo_peek");
                    prop_assert_eq!(
                        ground.is_some(),
                        db_step.is_some(),
                        "undo availability must agree"
                    );

                    if let (Some((step, new_pos)), Some(db_step)) = (ground, db_step) {
                        buf = apply_inverse(&buf, &step.edits).expect("apply_inverse");
                        journal.move_pos(new_pos);
                        move_undo_pos(&tx, session_id, doc_id, db_step.new_pos)
                            .expect("db move_undo_pos (undo)");
                    }
                    tx.commit().expect("commit");
                }
                Action::Redo => {
                    let ground = journal.redo_peek().map(|(step, pos)| (step.clone(), pos));

                    let tx = conn.transaction().expect("begin tx");
                    let db_step = redo_peek(&tx, session_id, doc_id).expect("db redo_peek");
                    prop_assert_eq!(
                        ground.is_some(),
                        db_step.is_some(),
                        "redo availability must agree"
                    );

                    if let (Some((step, new_pos)), Some(db_step)) = (ground, db_step) {
                        buf = reapply(&buf, &step.edits).expect("reapply");
                        journal.move_pos(new_pos);
                        move_undo_pos(&tx, session_id, doc_id, db_step.new_pos)
                            .expect("db move_undo_pos (redo)");
                    }
                    tx.commit().expect("commit");
                }
            }

            let tx = conn.transaction().expect("begin tx (verify)");
            let db_content = recover_document(&tx, session_id, doc_id).expect("recover_document");
            let db_pos = current_seq(&tx, session_id, doc_id).expect("db current_seq");
            tx.commit().expect("commit");

            prop_assert_eq!(
                buf.content(),
                db_content.as_str(),
                "content diverged at journal position {} (rune-db current_seq {})",
                journal.pos(),
                db_pos,
            );
        }
    }
}
