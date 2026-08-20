use super::*;
use crate::commands::test_support::selecting;
use rune_core::cursor::CursorSpec;
use rune_vfs::Mem;
use std::sync::Arc;

fn app_with(content: &str, cursor_offset: usize) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    let id = app.active;
    app.doc_mut(id).unwrap().cursors = CursorSet::new(cursor_offset.min(content.len()));
    app.doc_mut(id).unwrap().viewport.set_size(80, 23);
    app
}

#[test]
fn backspace_on_a_reversed_selection_removes_exactly_the_highlighted_range() {
    let mut app = app_with("hello world", 0);
    let id = app.active;
    selecting(&mut app, id, 5, 0);

    delete_left(&mut app, id);

    assert_eq!(app.doc(id).unwrap().buffer.content(), " world");
}

#[test]
fn typing_over_a_reversed_selection_replaces_exactly_the_highlighted_range() {
    let mut app = app_with("hello world", 0);
    let id = app.active;
    selecting(&mut app, id, 5, 0);

    insert_char(&mut app, id, 'x');

    assert_eq!(app.doc(id).unwrap().buffer.content(), "x world");
}

#[test]
fn typing_over_a_forward_selection_replaces_exactly_the_highlighted_range() {
    let mut app = app_with("hello world", 0);
    let id = app.active;
    selecting(&mut app, id, 0, 5);

    insert_char(&mut app, id, 'x');

    assert_eq!(app.doc(id).unwrap().buffer.content(), "x world");
}

/// End-to-end regression for the illegal-edit-set hazard
/// `edit_core::coalesce_touching_edits` closes at the batch-
/// construction chokepoint (see that module's own unit tests for the
/// narrower proof): two cursors at ADJACENT byte offsets — a
/// perfectly ordinary, non-merged `CursorSet` state — both pressing
/// Delete-forward in the same keystroke. Each cursor's own derived
/// delete range is one byte (`[0,1)` and `[1,2)`); those two ranges
/// touch. Without the fix, the journaled step holds two `AppliedEdit`s
/// that share the same post-edit `start`, and `redo` (which calls
/// `undo::reapply`) panics on its precondition `debug_assert!` in this
/// (debug) test build — `crates/rune-fuzz` artifact `no-panic-
/// 7f29861c`, checked in as `repros/no-panic-01.rune`, reached this
/// exact shape via multicursor-add-below + typing + Cmd+X (cut, which
/// deletes each cursor's whole current line when it has no selection
/// — the same "adjacent derived ranges" hazard applied to whole
/// lines instead of single runes).
#[test]
fn delete_right_with_adjacent_cursors_journals_one_edit_and_redoes_cleanly() {
    let mut app = app_with("abcd", 0);
    let id = app.active;
    let two = CursorSet::new(0).add(CursorSpec {
        position: 1,
        anchor: 1,
        desired_col: 0,
    });
    assert_eq!(
        two.len(),
        2,
        "fixture must really hold two adjacent cursors"
    );
    app.doc_mut(id).unwrap().cursors = two;

    delete_right(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "cd");
    assert_eq!(app.doc(id).unwrap().journal.len(), 1);
    let step_edit_starts: Vec<usize> = app
        .doc(id)
        .unwrap()
        .journal
        .undo_peek()
        .map(|(step, _)| step.edits.iter().map(|e| e.start).collect())
        .unwrap_or_default();
    assert_eq!(
        step_edit_starts.len(),
        1,
        "the two touching per-cursor deletes must be journaled as ONE edit, \
         not two sharing a post-edit start: {step_edit_starts:?}"
    );

    undo(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "abcd");

    // Panics here (debug_assert in `undo::reapply`) if the two
    // cursors' adjacent delete-right ranges were journaled as two
    // AppliedEdits sharing a post-edit `start` instead of being
    // coalesced into one.
    redo(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "cd");
}
