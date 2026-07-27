//! Clipboard commands (WP8): copy/cut write OSC 52 bytes into `Effects.raw`
//! (never a `Cmd` — plan Gotchas: "Cmds must never touch the terminal");
//! paste spawns the `pbpaste` `Cmd`. Port of
//! `pkg/ui/components/textedit/commands_clipboard.go`: `extractCopyText`
//! (:72), `copyEntireLine` (:102-110), `handlePasteContent` (:153-181).
//!
//! Workspace-coupled (plan WP1 decision 4): `copy`/`cut`/`handle_paste_
//! content` take `(app: &mut App, id: DocumentId)` — `cut`/`handle_paste_
//! content` bottom out in `commands::edit`, which touches `app.db`/dirty
//! bookkeeping.
//!
//! `Msg::Paste` (bracketed paste) and `Msg::ClipboardRead` (pbpaste) both
//! funnel through `handle_paste_content` — the single function every paste
//! source calls, so a terminal ⌘V and an in-app `super+v` can never double-
//! insert the same text (plan Gotchas: "Bracketed paste vs pbpaste double-
//! paste").

use rune_core::buffer::Buffer;
use rune_core::cursor::{Cursor, CursorSet};

use crate::app::App;
use crate::clipboard::{osc52_copy, pbpaste_cmd};
use crate::commands::edit;
use crate::commands::nav;
use crate::document::DocumentId;
use crate::runtime::Effects;

/// Port of `commands_clipboard.go:extractCopyText`. Single cursor (Phase
/// 1's only case): the selection text, or — with no selection — the whole
/// current line including its trailing newline (`copy_entire_line`).
/// Multi-cursor joins each cursor's selection-or-line with `\n`, ported for
/// `CursorSet` parity even though Phase 1 never actually produces more
/// than one cursor.
fn extract_copy_text(buf: &Buffer, cursors: &CursorSet) -> String {
    let all = cursors.all();
    match all.as_slice() {
        [] => String::new(),
        [c] => copy_text_for_cursor(buf, c),
        _ => all
            .iter()
            .map(|c| copy_text_for_cursor(buf, c))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn copy_text_for_cursor(buf: &Buffer, c: &Cursor) -> String {
    if c.has_selection() {
        let start = c.selection_start();
        let end = nav::selection_end_inclusive(c, buf);
        buf.slice(start, end).unwrap_or("").to_string()
    } else {
        copy_entire_line(buf, c.position)
    }
}

/// Port of `commands_clipboard.go:copyEntireLine`: the full line at
/// `offset`, including its trailing `\n` unless it's the buffer's last
/// line.
fn copy_entire_line(buf: &Buffer, offset: usize) -> String {
    let (start, end) = nav::line_range_incl_newline(buf, offset);
    buf.slice(start, end).unwrap_or("").to_string()
}

/// Port of `commands_clipboard.go:clipboardCopy`: never mutates the
/// buffer — pushes the OSC 52 write directly into `Effects.raw` (plan
/// Gotchas: "Cmds must never touch the terminal", exactly why this is
/// `raw` output, not a `Cmd`).
pub fn copy(app: &mut App, id: DocumentId, effects: &mut Effects) {
    let Some(doc) = app.doc(id) else { return };
    let text = extract_copy_text(&doc.buffer, &doc.cursors);
    if !text.is_empty() {
        effects.raw.push(osc52_copy(text.as_bytes()));
    }
}

/// Port of `commands_clipboard.go:clipboardCut`: the same copy text as
/// `copy`, computed BEFORE the delete (so it reflects what's being
/// removed), plus a journaled delete of the same range(s) via
/// `commands::edit::delete_selection_or_line` — reusing the existing
/// selection-replacing edit machinery rather than duplicating the batch-
/// apply/journal logic here. The OSC 52 write is pushed unconditionally
/// once there's text to send, independent of whether the delete itself
/// succeeds (mirrors Go's `command.Result` returning `Cmd:
/// clipboardWriteCmd(text)` alongside the edit operation unconditionally).
pub fn cut(app: &mut App, id: DocumentId, effects: &mut Effects) {
    let Some(doc) = app.doc(id) else { return };
    let text = extract_copy_text(&doc.buffer, &doc.cursors);
    if !text.is_empty() {
        effects.raw.push(osc52_copy(text.as_bytes()));
    }
    edit::delete_selection_or_line(app, id);
}

/// Port of `commands_clipboard.go:clipboardPaste`: no buffer mutation
/// here — just spawns the pbpaste `Cmd`; the actual insertion happens
/// later, when its `Msg::ClipboardRead` reply reaches
/// `handle_paste_content`.
pub fn paste(effects: &mut Effects) {
    effects.cmds.push(pbpaste_cmd());
}

/// Port of `commands_clipboard.go:handlePasteContent` (:153-181), the
/// single funnel `Msg::Paste` (bracketed paste) and `Msg::ClipboardRead`
/// (pbpaste) both call — see module docs. Read-only documents are NOT
/// guarded here (review finding F1): `edit::insert_text` bottoms out in
/// `commands::edit::commit_edit_batch`, the single chokepoint that rejects
/// every mutating command against a read-only `Document` — see its docs and
/// `Document::read_only`'s. Duplicating the check here would just be a
/// second copy that could silently drift from the real gate; the only
/// guard this function keeps is the empty-text early-out, which the
/// chokepoint doesn't (and shouldn't) special-case.
///
/// Multi-cursor line-distribution (Go: paste text with the same line count
/// as the cursor set spread one line per cursor) is NOT ported: Phase 1
/// runs a single cursor, so Go's `distribute` branch is unreachable here.
/// `edit::insert_text` already reproduces Go's non-distribute fallback —
/// the same whole text replacing every cursor's selection — which is the
/// only case that can occur in Phase 1.
pub fn handle_paste_content(app: &mut App, id: DocumentId, text: &str) {
    if text.is_empty() {
        return;
    }
    if app.doc(id).is_none_or(|doc| doc.cursors.is_empty()) {
        return;
    }
    edit::insert_text(app, id, text);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use rune_core::buffer::Buffer;
    use rune_core::cursor::{Cursor, CursorSet};
    use rune_vfs::Mem;
    use std::sync::Arc;

    fn app_with(content: &str, cursor_offset: usize) -> App {
        let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
        let id = app.active;
        app.doc_mut(id).unwrap().cursors = CursorSet::new(cursor_offset.min(content.len()));
        app.doc_mut(id).unwrap().viewport.set_size(80, 23);
        app
    }

    fn selecting(app: &mut App, id: DocumentId, anchor: usize, position: usize) {
        let primary = app.doc(id).unwrap().cursors.primary();
        app.doc_mut(id).unwrap().cursors = CursorSet::new_from(&[Cursor {
            anchor,
            position,
            ..primary
        }]);
    }

    fn expected_osc52(text: &str) -> Vec<u8> {
        osc52_copy(text.as_bytes())
    }

    #[test]
    fn copy_of_a_selection_emits_exactly_one_osc52_raw_chunk_with_the_selected_bytes() {
        let mut app = app_with("hello world", 0);
        let id = app.active;
        selecting(&mut app, id, 0, 5); // "hello"
        let mut effects = Effects::default();
        copy(&mut app, id, &mut effects);

        assert_eq!(effects.raw, vec![expected_osc52("hello")]);
        assert!(effects.cmds.is_empty(), "copy must never spawn a Cmd");
        assert_eq!(
            app.doc(id).unwrap().buffer.content(),
            "hello world",
            "copy must never mutate the buffer"
        );
    }

    #[test]
    fn copy_with_no_selection_copies_the_whole_line_including_its_trailing_newline() {
        let mut app = app_with("first\nsecond\nthird", 8); // caret inside "second"
        let id = app.active;
        let mut effects = Effects::default();
        copy(&mut app, id, &mut effects);

        assert_eq!(effects.raw, vec![expected_osc52("second\n")]);
    }

    #[test]
    fn copy_with_no_selection_on_the_last_line_has_no_trailing_newline() {
        let mut app = app_with("first\nsecond\nthird", 15); // caret inside "third", the last line
        let id = app.active;
        let mut effects = Effects::default();
        copy(&mut app, id, &mut effects);

        assert_eq!(effects.raw, vec![expected_osc52("third")]);
    }

    #[test]
    fn cut_removes_the_selection_journals_it_and_emits_the_same_osc52_payload() {
        let mut app = app_with("hello world", 0);
        let id = app.active;
        selecting(&mut app, id, 0, 5); // "hello"
        let mut effects = Effects::default();
        cut(&mut app, id, &mut effects);

        assert_eq!(effects.raw, vec![expected_osc52("hello")]);
        assert_eq!(app.doc(id).unwrap().buffer.content(), " world");
        assert_eq!(app.doc(id).unwrap().journal.len(), 1);

        edit::undo(&mut app, id);
        assert_eq!(
            app.doc(id).unwrap().buffer.content(),
            "hello world",
            "undo must restore what cut removed"
        );
    }

    #[test]
    fn cut_with_no_selection_removes_the_whole_line_including_its_newline() {
        let mut app = app_with("first\nsecond\nthird", 8);
        let id = app.active;
        let mut effects = Effects::default();
        cut(&mut app, id, &mut effects);

        assert_eq!(effects.raw, vec![expected_osc52("second\n")]);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "first\nthird");
    }

    /// Regression for F1: on a read-only `Document`, Cut must not mutate
    /// the buffer, bump its version, or journal anything — the deletion is
    /// rejected at `commands::edit::commit_edit_batch`, the shared
    /// chokepoint every mutating command (including cut) funnels through.
    /// The OSC 52 copy itself is a read, not a mutation, and is unaffected.
    #[test]
    fn cut_on_a_read_only_editor_does_not_mutate_the_buffer() {
        let mut app = app_with("hello world", 0);
        let id = app.active;
        selecting(&mut app, id, 0, 5); // "hello"
        app.doc_mut(id).unwrap().read_only = true;
        let before_version = app.doc(id).unwrap().buffer.version();

        let mut effects = Effects::default();
        cut(&mut app, id, &mut effects);

        assert_eq!(app.doc(id).unwrap().buffer.content(), "hello world");
        assert_eq!(app.doc(id).unwrap().buffer.version(), before_version);
        assert_eq!(app.doc(id).unwrap().journal.len(), 0);
    }

    #[test]
    fn paste_spawns_exactly_one_pbpaste_cmd_and_never_touches_the_buffer_or_raw() {
        let mut effects = Effects::default();
        paste(&mut effects);

        assert_eq!(effects.cmds.len(), 1);
        assert!(effects.raw.is_empty(), "paste must never emit raw bytes");
    }

    #[test]
    fn handle_paste_content_inserts_at_the_caret_and_journals_it() {
        let mut app = app_with("ac", 1);
        let id = app.active;
        handle_paste_content(&mut app, id, "b");

        assert_eq!(app.doc(id).unwrap().buffer.content(), "abc");
        assert_eq!(app.doc(id).unwrap().cursors.primary().position, 2);
        assert_eq!(app.doc(id).unwrap().journal.len(), 1);
    }

    #[test]
    fn bracketed_paste_and_clipboard_read_funnel_through_the_same_insertion_path() {
        use crate::app;
        use crate::runtime::Msg;

        let mut paste_app = app_with("ac", 1);
        let paste_id = paste_app.active;
        let mut effects = Effects::default();
        app::update(&mut paste_app, Msg::Paste("b".to_string()), &mut effects);

        let mut read_app = app_with("ac", 1);
        let read_id = read_app.active;
        let mut effects2 = Effects::default();
        app::update(
            &mut read_app,
            Msg::ClipboardRead("b".to_string()),
            &mut effects2,
        );

        assert_eq!(paste_app.doc(paste_id).unwrap().buffer.content(), "abc");
        assert_eq!(
            paste_app.doc(paste_id).unwrap().buffer.content(),
            read_app.doc(read_id).unwrap().buffer.content(),
            "Msg::Paste and Msg::ClipboardRead must produce identical results"
        );
        assert_eq!(
            paste_app.doc(paste_id).unwrap().cursors.primary().position,
            read_app.doc(read_id).unwrap().cursors.primary().position
        );
        assert_eq!(
            paste_app.doc(paste_id).unwrap().journal.len(),
            read_app.doc(read_id).unwrap().journal.len()
        );
    }

    #[test]
    fn handle_paste_content_is_a_no_op_on_a_read_only_editor() {
        let mut app = app_with("ac", 1);
        let id = app.active;
        app.doc_mut(id).unwrap().read_only = true;
        handle_paste_content(&mut app, id, "b");

        assert_eq!(app.doc(id).unwrap().buffer.content(), "ac");
        assert_eq!(app.doc(id).unwrap().journal.len(), 0);
    }

    #[test]
    fn handle_paste_content_of_empty_text_is_a_no_op() {
        let mut app = app_with("ac", 1);
        let id = app.active;
        handle_paste_content(&mut app, id, "");

        assert_eq!(app.doc(id).unwrap().buffer.content(), "ac");
        assert_eq!(app.doc(id).unwrap().journal.len(), 0);
    }
}
