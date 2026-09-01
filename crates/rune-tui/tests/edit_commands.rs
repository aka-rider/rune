//! `commands::edit`'s per-cursor editing commands and undo/redo (moved out
//! of `edit.rs` to keep that file under the 500-line
//! budget — every item exercised here (`App`, `commands::edit`'s `pub`
//! functions, `CursorSet`) is already public, so this needs no
//! crate-internal access `#[cfg(test)]` had; the same pattern
//! `tests/app_quit_and_dispatch.rs` used for `app.rs`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::coords::BufferOffset;
use rune_core::cursor::{Cursor, CursorSet};
use rune_tui::app::App;
use rune_tui::commands::edit::{
    delete_left, delete_right, delete_word_left, delete_word_right, insert_char, newline, redo,
    undo,
};
use rune_tui::document::ReadOnly;
use rune_vfs::Mem;

fn app_with(content: &str, cursor_offset: usize) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    let id = app.active;
    app.doc_mut(id).unwrap().cursors = CursorSet::new(cursor_offset.min(content.len()));
    app.doc_mut(id).unwrap().viewport.set_size(80, 23);
    app
}

#[test]
fn insert_char_moves_caret_past_the_inserted_char() {
    let mut app = app_with("ac", 1);
    let id = app.active;
    insert_char(&mut app, id, 'b');
    assert_eq!(app.doc(id).unwrap().buffer.content(), "abc");
    assert_eq!(
        app.doc(id).unwrap().cursors.primary().position,
        BufferOffset(2)
    );
    assert_eq!(app.doc(id).unwrap().journal.len(), 1);
}

#[test]
fn insert_char_replaces_a_selection() {
    let mut app = app_with("hello world", 0);
    let id = app.active;
    app.doc_mut(id).unwrap().cursors = CursorSet::new(0).map(|c| Cursor {
        anchor: BufferOffset(0),
        position: BufferOffset(5),
        ..c
    });
    insert_char(&mut app, id, 'X');
    assert_eq!(app.doc(id).unwrap().buffer.content(), "X world");
    assert_eq!(
        app.doc(id).unwrap().cursors.primary().position,
        BufferOffset(1)
    );
}

#[test]
fn backspace_deletes_one_rune_never_splitting_a_multibyte_char() {
    let mut app = app_with("a\u{6c49}", "a".len() + '\u{6c49}'.len_utf8());
    let id = app.active;
    delete_left(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "a");
}

#[test]
fn backspace_at_buffer_start_is_a_no_op() {
    let mut app = app_with("abc", 0);
    let id = app.active;
    delete_left(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "abc");
    assert_eq!(app.doc(id).unwrap().journal.len(), 0);
}

#[test]
fn delete_right_removes_one_rune_forward() {
    let mut app = app_with("abc", 0);
    let id = app.active;
    delete_right(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "bc");
    assert_eq!(
        app.doc(id).unwrap().cursors.primary().position,
        BufferOffset(0)
    );
}

#[test]
fn delete_word_left_removes_the_whole_preceding_word() {
    let mut app = app_with("hello world", 11);
    let id = app.active;
    delete_word_left(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "hello ");
    assert_eq!(
        app.doc(id).unwrap().cursors.primary().position,
        BufferOffset(6)
    );
}

#[test]
fn delete_word_left_at_buffer_start_is_a_no_op() {
    let mut app = app_with("hello", 0);
    let id = app.active;
    delete_word_left(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "hello");
    assert_eq!(app.doc(id).unwrap().journal.len(), 0);
}

/// Undo restores the buffer byte-for-byte — the property the fuzzer's
/// `UNDO-TOTAL` invariant checks across a whole session.
#[test]
fn delete_word_left_then_undo_restores_the_buffer() {
    let mut app = app_with("hello world", 11);
    let id = app.active;
    let original = app.doc(id).unwrap().buffer.content().to_string();
    delete_word_left(&mut app, id);
    undo(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), original);
}

#[test]
fn delete_word_right_removes_the_whole_following_word() {
    let mut app = app_with("hello world", 0);
    let id = app.active;
    delete_word_right(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), " world");
}

#[test]
fn delete_word_right_at_buffer_end_is_a_no_op() {
    let mut app = app_with("hello", 5);
    let id = app.active;
    delete_word_right(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "hello");
    assert_eq!(app.doc(id).unwrap().journal.len(), 0);
}

#[test]
fn delete_word_right_then_undo_restores_the_buffer() {
    let mut app = app_with("hello world", 0);
    let id = app.active;
    let original = app.doc(id).unwrap().buffer.content().to_string();
    delete_word_right(&mut app, id);
    undo(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), original);
}

#[test]
fn newline_preserves_current_line_indentation() {
    let mut app = app_with("  indented", 10);
    let id = app.active;
    newline(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "  indented\n  ");
    assert_eq!(
        app.doc(id).unwrap().cursors.primary().position,
        BufferOffset(13)
    );
}

#[test]
fn undo_then_redo_round_trips_content_and_cursors() {
    let mut app = app_with("hello", 5);
    let id = app.active;
    insert_char(&mut app, id, '!');
    assert_eq!(app.doc(id).unwrap().buffer.content(), "hello!");

    undo(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "hello");
    assert_eq!(
        app.doc(id).unwrap().cursors.primary().position,
        BufferOffset(5)
    );

    redo(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "hello!");
    assert_eq!(
        app.doc(id).unwrap().cursors.primary().position,
        BufferOffset(6)
    );
}

#[test]
fn undo_with_empty_journal_is_a_no_op() {
    let mut app = app_with("hello", 0);
    let id = app.active;
    undo(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "hello");
}

#[test]
fn undo_at_the_journal_start_posts_nothing_to_undo() {
    let mut app = app_with("hello", 0);
    let id = app.active;
    undo(&mut app, id);
    assert_eq!(
        rune_tui::messages::newest_text(&app),
        Some("nothing to undo"),
        "undoing an empty journal must surface a status message"
    );
}

#[test]
fn redo_with_nothing_ahead_posts_nothing_to_redo() {
    let mut app = app_with("hello", 5);
    let id = app.active;
    insert_char(&mut app, id, '!');
    redo(&mut app, id);
    assert_eq!(
        rune_tui::messages::newest_text(&app),
        Some("nothing to redo"),
        "redoing with nothing ahead must surface a status message"
    );
}

#[test]
fn cjk_and_emoji_round_trip_byte_exact_through_undo() {
    let mut app = app_with("汉字 👩‍👩‍👧‍👦", 0);
    let id = app.active;
    let original = app.doc(id).unwrap().buffer.content().to_string();
    insert_char(&mut app, id, 'x');
    undo(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), original);
}

/// The log is append-only: a successful edit/undo/redo posts nothing at
/// all, so an unrelated
/// (e.g. save-failure) entry an earlier subsystem posted simply stays put —
/// there is no shared slot left to clobber.
#[test]
fn a_successful_edit_keeps_an_unrelated_log_entry() {
    let mut app = app_with("hello", 5);
    let id = app.active;
    rune_tui::messages::error(&mut app, "save failed: disk full");

    insert_char(&mut app, id, '!');
    assert_eq!(app.doc(id).unwrap().buffer.content(), "hello!");
    assert_eq!(
        rune_tui::messages::log_text(&app),
        "save failed: disk full",
        "an unrelated save-failure entry must survive a successful edit"
    );

    undo(&mut app, id);
    assert_eq!(
        rune_tui::messages::log_text(&app),
        "save failed: disk full",
        "an unrelated save-failure entry must survive a successful undo"
    );

    redo(&mut app, id);
    assert_eq!(
        rune_tui::messages::log_text(&app),
        "save failed: disk full",
        "an unrelated save-failure entry must survive a successful redo"
    );
}

/// Regression for F1: a read-only `Document` rejects every mutating
/// command at the `apply_edit_batch_with_cursors` chokepoint — buffer
/// content, buffer version, and the undo journal are all left untouched by
/// typing, Backspace, delete-right and newline alike (an earlier version
/// guarded only `commands::clipboard::handle_paste_content`, leaving these
/// paths able to mutate a "read-only" document). The line-oriented
/// commands (indent/outdent/delete-line/clone/move-line) share the SAME
/// chokepoint but live in `edit_lines` — see its own
/// `read_only_blocks_line_commands` for that half of this regression.
#[test]
fn read_only_blocks_typing_backspace_and_newline() {
    let mut app = app_with("hello", 5);
    let id = app.active;
    app.doc_mut(id).unwrap().read_only = ReadOnly::Always;
    let before_content = app.doc(id).unwrap().buffer.content().to_string();
    let before_version = app.doc(id).unwrap().buffer.version();

    insert_char(&mut app, id, '!');
    assert_eq!(
        app.doc(id).unwrap().buffer.content(),
        before_content,
        "typing"
    );

    delete_left(&mut app, id);
    assert_eq!(
        app.doc(id).unwrap().buffer.content(),
        before_content,
        "backspace"
    );

    delete_right(&mut app, id);
    assert_eq!(
        app.doc(id).unwrap().buffer.content(),
        before_content,
        "delete-right"
    );

    newline(&mut app, id);
    assert_eq!(
        app.doc(id).unwrap().buffer.content(),
        before_content,
        "newline"
    );

    assert_eq!(
        app.doc(id).unwrap().buffer.version(),
        before_version,
        "a rejected mutation must never bump the buffer version"
    );
    assert_eq!(
        app.doc(id).unwrap().journal.len(),
        0,
        "a rejected mutation must never be journaled"
    );
}

/// Regression for F1: `undo`/`redo` are deliberately NOT
/// gated by `read_only`, the same way `ReplaceRange` is not. A document that
/// became read-only after edits were already journaled (e.g. the Help
/// view is generated fresh and never has journal history, but this
/// property must hold regardless) must still let undo/redo walk that
/// history.
#[test]
fn undo_and_redo_are_not_blocked_by_read_only() {
    let mut app = app_with("hello", 5);
    let id = app.active;
    insert_char(&mut app, id, '!');
    assert_eq!(app.doc(id).unwrap().buffer.content(), "hello!");

    app.doc_mut(id).unwrap().read_only = ReadOnly::Always;

    undo(&mut app, id);
    assert_eq!(
        app.doc(id).unwrap().buffer.content(),
        "hello",
        "undo must not be blocked by read_only"
    );

    redo(&mut app, id);
    assert_eq!(
        app.doc(id).unwrap().buffer.content(),
        "hello!",
        "redo must not be blocked by read_only"
    );
}

/// The asymmetry `ReadOnly::Reading` draws: unlike `Always`
/// (asserted above), a reading-view document blocks BOTH undo and redo —
/// it is a view mode the user can leave with the same chord, not a
/// document with no editable form at all. Neither the buffer nor
/// dirtiness may move while blocked.
#[test]
fn reading_view_blocks_undo_and_redo() {
    let mut app = app_with("hello", 5);
    let id = app.active;
    insert_char(&mut app, id, '!');
    assert_eq!(app.doc(id).unwrap().buffer.content(), "hello!");

    app.doc_mut(id).unwrap().read_only = ReadOnly::Reading;
    let dirty_before = app.doc(id).unwrap().is_dirty();

    undo(&mut app, id);
    assert_eq!(
        app.doc(id).unwrap().buffer.content(),
        "hello!",
        "undo must be blocked in reading view"
    );
    assert_eq!(
        app.doc(id).unwrap().is_dirty(),
        dirty_before,
        "a blocked undo must not change dirtiness"
    );
    assert_eq!(
        rune_tui::messages::newest_text(&app),
        ReadOnly::Reading.refusal_message(),
        "a reading-view undo refusal must surface a status message"
    );

    redo(&mut app, id);
    assert_eq!(
        app.doc(id).unwrap().buffer.content(),
        "hello!",
        "redo must be blocked in reading view"
    );
    assert_eq!(
        app.doc(id).unwrap().is_dirty(),
        dirty_before,
        "a blocked redo must not change dirtiness"
    );
    assert_eq!(
        rune_tui::messages::newest_text(&app),
        ReadOnly::Reading.refusal_message(),
        "a reading-view redo refusal must surface a status message"
    );
}
