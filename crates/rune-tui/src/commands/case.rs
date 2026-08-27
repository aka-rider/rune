use rune_core::undo::EditKind;

use crate::app::App;
use crate::commands::edit::per_cursor_selection_edits;
use crate::commands::nav;
use crate::document::DocumentId;
use crate::messages;

#[derive(Clone, Copy, PartialEq, Eq)]
enum CaseOp {
    Upper,
    Lower,
}

fn has_actionable_range(app: &App, id: DocumentId) -> bool {
    let Some(doc) = app.doc(id) else { return false };
    doc.cursors.all().iter().any(|c| {
        if c.has_selection() {
            let (start, end) = c.selection_range();
            start != end
        } else {
            let (start, end) = nav::word_range_at(&doc.buffer, c.position.get());
            start != end && nav::is_word_at(&doc.buffer, start)
        }
    })
}

fn apply_case(app: &mut App, id: DocumentId, op: CaseOp) {
    if !has_actionable_range(app, id) {
        messages::info(app, "no word under cursor");
        return;
    }
    per_cursor_selection_edits(
        app,
        id,
        EditKind::Other,
        move |_i, c, buf| {
            let (start, end) = if c.has_selection() {
                let (s, e) = c.selection_range();
                (s.get(), e.get())
            } else {
                nav::word_range_at(buf, c.position.get())
            };
            let text = buf.slice(start, end).unwrap_or("");
            match op {
                CaseOp::Upper => text.to_uppercase(),
                CaseOp::Lower => text.to_lowercase(),
            }
        },
        |buf, c| {
            let (start, end) = nav::word_range_at(buf, c.position.get());
            if start == end || !nav::is_word_at(buf, start) {
                None
            } else {
                Some((start, end))
            }
        },
    );
}

pub fn uppercase(app: &mut App, id: DocumentId) {
    apply_case(app, id, CaseOp::Upper);
}

pub fn lowercase(app: &mut App, id: DocumentId) {
    apply_case(app, id, CaseOp::Lower);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use std::sync::Arc;

    use rune_core::buffer::Buffer;
    use rune_core::coords::{BufferOffset, VisualCol};
    use rune_core::cursor::{CursorSet, CursorSpec};
    use rune_vfs::Mem;

    use super::*;
    use crate::commands::test_support::selecting;
    use crate::document::ReadOnly;

    fn app_with(content: &str, cursor_offset: usize) -> App {
        let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
        let id = app.active;
        app.doc_mut(id).unwrap().cursors = CursorSet::new(cursor_offset.min(content.len()));
        app.doc_mut(id).unwrap().viewport.set_size(80, 23);
        app
    }

    #[test]
    fn uppercase_transforms_the_active_selection() {
        let mut app = app_with("hello world", 0);
        let id = app.active;
        selecting(&mut app, id, 0, 5);

        uppercase(&mut app, id);

        assert_eq!(app.doc(id).unwrap().buffer.content(), "HELLO world");
    }

    #[test]
    fn lowercase_falls_back_to_the_word_under_the_cursor() {
        let mut app = app_with("HELLO world", 2);
        let id = app.active;

        lowercase(&mut app, id);

        assert_eq!(app.doc(id).unwrap().buffer.content(), "hello world");
    }

    #[test]
    fn multiple_cursors_each_transform_their_own_word() {
        let mut app = app_with("foo bar", 1);
        let id = app.active;
        let two = CursorSet::new(1).add(CursorSpec {
            position: BufferOffset(5),
            anchor: BufferOffset(5),
            desired_col: VisualCol(0),
        });
        app.doc_mut(id).unwrap().cursors = two;

        uppercase(&mut app, id);

        assert_eq!(app.doc(id).unwrap().buffer.content(), "FOO BAR");
    }

    #[test]
    fn two_cursors_in_one_word_uppercase_as_a_single_coalesced_edit() {
        let mut app = app_with("hello world", 2);
        let id = app.active;
        let two = CursorSet::new(2).add(CursorSpec {
            position: BufferOffset(3),
            anchor: BufferOffset(3),
            desired_col: VisualCol(0),
        });
        app.doc_mut(id).unwrap().cursors = two;

        uppercase(&mut app, id);

        assert_eq!(app.doc(id).unwrap().buffer.content(), "HELLO world");
        let steps = app.doc(id).unwrap().journal.steps();
        assert_eq!(
            steps.len(),
            1,
            "the uppercase press must journal exactly one step"
        );
        assert_eq!(
            steps[0].edits.len(),
            1,
            "two cursors resolving to the same word must coalesce into one edit, \
             not two identical edits that collide on the same range"
        );
    }

    #[test]
    fn punctuation_under_the_cursor_refuses_with_a_visible_message() {
        let mut app = app_with("foo, bar", 3);
        let id = app.active;

        uppercase(&mut app, id);

        assert_eq!(app.doc(id).unwrap().buffer.content(), "foo, bar");
        assert!(messages::log_text(&app).contains("no word under cursor"));
    }

    #[test]
    fn a_read_only_document_refuses_the_edit() {
        let mut app = app_with("hello world", 0);
        let id = app.active;
        app.doc_mut(id).unwrap().read_only = ReadOnly::Reading;

        uppercase(&mut app, id);

        assert_eq!(app.doc(id).unwrap().buffer.content(), "hello world");
    }

    #[test]
    fn one_uppercase_press_undoes_in_a_single_step() {
        let mut app = app_with("hello world", 0);
        let id = app.active;
        selecting(&mut app, id, 0, 5);

        uppercase(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "HELLO world");

        crate::commands::edit::undo(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "hello world");
    }
}
