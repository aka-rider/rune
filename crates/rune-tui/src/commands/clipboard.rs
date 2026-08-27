use rune_core::buffer::Buffer;
use rune_core::cursor::{Cursor, CursorSet};

use crate::app::App;
use crate::clipboard::{OSC52_MAX_PAYLOAD_BYTES, osc52_copy, pbpaste_cmd};
use crate::commands::edit;
use crate::commands::nav_line;
use crate::document::DocumentId;
use crate::messages;
use crate::pane::Pane;
use crate::runtime::{Effects, PasteTarget};

pub(crate) fn write_to_clipboard_or_report(app: &mut App, text: &str, effects: &mut Effects) {
    if text.is_empty() {
        return;
    }
    if text.len() > OSC52_MAX_PAYLOAD_BYTES {
        messages::error(
            app,
            format!(
                "selection too large to copy to the system clipboard \
                 ({} bytes, limit {OSC52_MAX_PAYLOAD_BYTES})",
                text.len()
            ),
        );
        return;
    }
    effects.write(osc52_copy(text.as_bytes()));
}

pub(crate) fn extract_copy_text(buf: &Buffer, cursors: &CursorSet) -> String {
    let all = cursors.all();
    match all {
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
        let (start, end) = c.selection_range();
        buf.slice(start.get(), end.get()).unwrap_or("").to_string()
    } else {
        copy_entire_line(buf, c.position.get())
    }
}

fn copy_entire_line(buf: &Buffer, offset: usize) -> String {
    let (start, end) = nav_line::line_range_incl_newline(buf, offset);
    buf.slice(start, end).unwrap_or("").to_string()
}

pub fn copy(app: &mut App, id: DocumentId, effects: &mut Effects) {
    let Some(doc) = app.doc(id) else { return };
    let text = extract_copy_text(&doc.buffer, &doc.cursors);
    write_to_clipboard_or_report(app, &text, effects);
}

pub fn cut(app: &mut App, id: DocumentId, effects: &mut Effects) {
    let Some(doc) = app.doc(id) else { return };
    let text = extract_copy_text(&doc.buffer, &doc.cursors);
    write_to_clipboard_or_report(app, &text, effects);
    edit::delete_selection_or_line(app, id);
}

pub fn paste(effects: &mut Effects, target: PasteTarget) {
    effects.cmds.push(pbpaste_cmd(target));
}

pub fn handle_paste_content(app: &mut App, id: DocumentId, text: &str) {
    if text.is_empty() {
        return;
    }
    if app.doc(id).is_none_or(|doc| doc.cursors.is_empty()) {
        return;
    }
    edit::insert_text(app, id, text, rune_core::undo::EditKind::Paste);
}

/// Deliberately not gated on `app.guard`, unlike the key pipeline's stage
/// 1: a paste carries user content, and dropping it because a prompt
/// happens to be up discards something the user explicitly asked to
/// insert — the buffer is journaled and undoable, so landing it there is
/// the safer failure mode than losing it.
pub(crate) fn route_bracketed_paste(app: &mut App, text: &str, effects: &mut Effects) {
    match crate::focus::target(app) {
        crate::focus::FocusTarget::SearchField => crate::search::keys::paste(app, text),
        crate::focus::FocusTarget::FileSearch => crate::filesearch::keys::paste(app, text, effects),
        crate::focus::FocusTarget::Palette => crate::palette::keys::paste(app, text),
        crate::focus::FocusTarget::ReplaceField => handle_paste_content(app, app.active, text),
        crate::focus::FocusTarget::Explorer
        | crate::focus::FocusTarget::Tabs
        | crate::focus::FocusTarget::Editor
        | crate::focus::FocusTarget::Title
        | crate::focus::FocusTarget::Messages => match app.focus() {
            Pane::Title => crate::title::keys::paste(app, app.active, text),
            Pane::Editor => handle_paste_content(app, app.active, text),
            Pane::Explorer | Pane::Tabs | Pane::Messages => {
                messages::warn(app, "nothing to paste into here");
            }
        },
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::commands::test_support::selecting;
    use rune_core::buffer::Buffer;
    use rune_core::cursor::CursorSet;
    use rune_vfs::Mem;
    use std::sync::Arc;

    fn app_with(content: &str, cursor_offset: usize) -> App {
        let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
        let id = app.active;
        app.doc_mut(id).unwrap().cursors = CursorSet::new(cursor_offset.min(content.len()));
        app.doc_mut(id).unwrap().viewport.set_size(80, 23);
        app
    }

    fn expected_osc52(text: &str) -> Vec<u8> {
        osc52_copy(text.as_bytes())
    }

    #[test]
    fn copy_of_a_selection_emits_exactly_one_osc52_raw_chunk_with_the_selected_bytes() {
        let mut app = app_with("hello world", 0);
        let id = app.active;
        selecting(&mut app, id, 0, 5);
        let mut effects = Effects::default();
        copy(&mut app, id, &mut effects);

        assert_eq!(effects.raw_bytes(), vec![expected_osc52("hello")]);
        assert!(effects.cmds.is_empty(), "copy must never spawn a Cmd");
        assert_eq!(
            app.doc(id).unwrap().buffer.content(),
            "hello world",
            "copy must never mutate the buffer"
        );
    }

    #[test]
    fn copy_with_no_selection_copies_the_whole_line_including_its_trailing_newline() {
        let mut app = app_with("first\nsecond\nthird", 8);
        let id = app.active;
        let mut effects = Effects::default();
        copy(&mut app, id, &mut effects);

        assert_eq!(effects.raw_bytes(), vec![expected_osc52("second\n")]);
    }

    #[test]
    fn copy_with_no_selection_on_the_last_line_has_no_trailing_newline() {
        let mut app = app_with("first\nsecond\nthird", 15);
        let id = app.active;
        let mut effects = Effects::default();
        copy(&mut app, id, &mut effects);

        assert_eq!(effects.raw_bytes(), vec![expected_osc52("third")]);
    }

    #[test]
    fn copy_of_a_reversed_selection_emits_exactly_the_highlighted_bytes() {
        let mut app = app_with("hello world", 5);
        let id = app.active;
        selecting(&mut app, id, 5, 0);
        let mut effects = Effects::default();
        copy(&mut app, id, &mut effects);

        assert_eq!(effects.raw_bytes(), vec![expected_osc52("hello")]);
    }

    #[test]
    fn cut_removes_the_selection_journals_it_and_emits_the_same_osc52_payload() {
        let mut app = app_with("hello world", 0);
        let id = app.active;
        selecting(&mut app, id, 0, 5);
        let mut effects = Effects::default();
        cut(&mut app, id, &mut effects);

        assert_eq!(effects.raw_bytes(), vec![expected_osc52("hello")]);
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
    fn copy_over_the_osc52_cap_posts_a_message_instead_of_writing_raw() {
        let huge = "x".repeat(OSC52_MAX_PAYLOAD_BYTES + 1);
        let mut app = app_with(&huge, 0);
        let id = app.active;
        selecting(&mut app, id, 0, huge.len());
        let mut effects = Effects::default();
        copy(&mut app, id, &mut effects);

        assert!(
            effects.raw_bytes().is_empty(),
            "an over-cap selection must never reach the OSC 52 raw output"
        );
        assert!(
            crate::messages::newest_text(&app).is_some(),
            "an over-cap copy must post a message reporting the failure"
        );
    }

    #[test]
    fn cut_over_the_osc52_cap_posts_a_message_but_still_deletes() {
        let huge = "x".repeat(OSC52_MAX_PAYLOAD_BYTES + 1);
        let mut app = app_with(&huge, 0);
        let id = app.active;
        selecting(&mut app, id, 0, huge.len());
        let mut effects = Effects::default();
        cut(&mut app, id, &mut effects);

        assert!(
            effects.raw_bytes().is_empty(),
            "an over-cap selection must never reach the OSC 52 raw output"
        );
        assert!(
            crate::messages::newest_text(&app).is_some(),
            "an over-cap cut must post a message reporting the failure"
        );
        assert_eq!(
            app.doc(id).unwrap().buffer.content(),
            "",
            "the delete must still proceed even when the clipboard write is refused"
        );
    }

    #[test]
    fn cut_with_no_selection_removes_the_whole_line_including_its_newline() {
        let mut app = app_with("first\nsecond\nthird", 8);
        let id = app.active;
        let mut effects = Effects::default();
        cut(&mut app, id, &mut effects);

        assert_eq!(effects.raw_bytes(), vec![expected_osc52("second\n")]);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "first\nthird");
    }

    #[test]
    fn cut_on_a_read_only_editor_does_not_mutate_the_buffer() {
        let mut app = app_with("hello world", 0);
        let id = app.active;
        selecting(&mut app, id, 0, 5);
        app.doc_mut(id).unwrap().read_only = crate::document::ReadOnly::Always;
        let before_version = app.doc(id).unwrap().buffer.version();

        let mut effects = Effects::default();
        cut(&mut app, id, &mut effects);

        assert_eq!(app.doc(id).unwrap().buffer.content(), "hello world");
        assert_eq!(app.doc(id).unwrap().buffer.version(), before_version);
        assert_eq!(app.doc(id).unwrap().journal.len(), 0);
    }

    #[test]
    fn paste_spawns_exactly_one_pbpaste_cmd_and_never_touches_the_buffer_or_raw() {
        let app = app_with("ac", 1);
        let mut effects = Effects::default();
        paste(&mut effects, PasteTarget::Document(app.active));

        assert_eq!(effects.cmds.len(), 1);
        assert!(
            effects.raw_bytes().is_empty(),
            "paste must never emit raw bytes"
        );
    }

    #[test]
    fn handle_paste_content_inserts_at_the_caret_and_journals_it() {
        let mut app = app_with("ac", 1);
        let id = app.active;
        handle_paste_content(&mut app, id, "b");

        assert_eq!(app.doc(id).unwrap().buffer.content(), "abc");
        assert_eq!(
            app.doc(id).unwrap().cursors.primary().position,
            rune_core::coords::BufferOffset(2)
        );
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
            Msg::ClipboardRead {
                text: "b".to_string(),
                target: PasteTarget::Document(read_id),
            },
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
    fn bracketed_paste_while_the_search_bar_is_focused_lands_in_the_draft() {
        use crate::app;
        use crate::runtime::Msg;

        let mut app = app_with("ac", 1);
        crate::search::open(&mut app, &mut crate::runtime::Effects::default());
        let id = app.active;
        let before = app.doc(id).unwrap().buffer.content().to_string();

        let mut effects = Effects::default();
        app::update(&mut app, Msg::Paste("b".to_string()), &mut effects);

        assert_eq!(
            app.doc(id).unwrap().buffer.content(),
            before,
            "the document buffer must be untouched while the bar is focused"
        );
        assert_eq!(app.search().unwrap().draft, "b");
    }

    #[test]
    fn handle_paste_content_is_a_no_op_on_a_read_only_editor() {
        let mut app = app_with("ac", 1);
        let id = app.active;
        app.doc_mut(id).unwrap().read_only = crate::document::ReadOnly::Always;
        handle_paste_content(&mut app, id, "b");

        assert_eq!(app.doc(id).unwrap().buffer.content(), "ac");
        assert_eq!(app.doc(id).unwrap().journal.len(), 0);
    }

    #[test]
    fn bracketed_paste_while_a_non_editor_pane_is_focused_refuses_with_feedback() {
        use crate::app;
        use crate::runtime::Msg;

        for pane in [Pane::Explorer, Pane::Tabs, Pane::Messages] {
            let mut app = app_with("ac", 1);
            app.frame = Some(crate::app::FrameSize::new(120, 34));
            app.splits.left.show();
            if pane == Pane::Messages {
                crate::messages::toggle(&mut app, &mut Effects::default());
            }
            app.set_focus_pane(pane, &mut Effects::default());
            assert_eq!(app.focus(), pane, "test setup: {pane:?} must be focusable");

            let mut effects = Effects::default();
            app::update(&mut app, Msg::Paste("b".to_string()), &mut effects);

            assert_eq!(
                app.doc(app.active).unwrap().buffer.content(),
                "ac",
                "{pane:?} focused must never let bracketed paste reach the editor's document"
            );
            assert_eq!(
                crate::messages::newest_text(&app),
                Some("nothing to paste into here"),
                "{pane:?} focused must refuse the paste with feedback"
            );
        }
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
