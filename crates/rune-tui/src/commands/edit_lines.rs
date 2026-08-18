use std::collections::HashSet;

use rune_core::buffer::{Buffer, Edit};
use rune_core::cursor::{Cursor, CursorId};
use rune_core::undo::EditKind;

use crate::app::App;
use crate::commands::edit_core::{apply_edit_batch_with_cursors, commit_edit_batch};
use crate::document::DocumentId;

/// `dedupe=true` (delete-line) skips a line an earlier cursor in this same
/// batch already produced an edit for — two cursors on one line must not
/// double-edit it. `dedupe=false` (clone-line-up/down, in the sibling
/// `edit_lines_move` module) lets every cursor clone independently even
/// when several cursors share a line.
pub(crate) fn per_line_edits(
    app: &mut App,
    id: DocumentId,
    dedupe: bool,
    build: impl Fn(usize, &Buffer) -> Option<Edit>,
) {
    let Some(doc) = app.doc(id) else { return };
    let cursors_before = doc.cursors.clone();
    let all = cursors_before.all();
    if all.is_empty() {
        return;
    }

    let mut infos: Vec<(Edit, CursorId)> = Vec::new();
    let mut seen: HashSet<usize> = HashSet::new();
    for c in all {
        let Some(doc) = app.doc(id) else { return };
        let bp = doc.buffer.offset_to_line_col(c.position);
        if dedupe && !seen.insert(bp.line) {
            continue;
        }
        let Some(doc) = app.doc(id) else { return };
        if let Some(edit) = build(bp.line, &doc.buffer) {
            infos.push((edit, c.id));
        }
    }

    let _ = commit_edit_batch(app, id, infos, &cursors_before, EditKind::Other);
}

fn selected_lines(c: &Cursor, buf: &Buffer) -> std::ops::RangeInclusive<usize> {
    let first = buf.offset_to_line_col(c.selection_start()).line;
    let mut last = buf.offset_to_line_col(c.selection_end()).line;
    if last > first && buf.line_start(last) == Some(c.selection_end()) {
        last -= 1;
    }
    first..=last
}

struct LineShift {
    start: usize,
    removed: usize,
    inserted: usize,
}

fn shift_through(offset: usize, shifts: &[LineShift]) -> usize {
    let mut out = offset;
    for s in shifts {
        if offset <= s.start {
            break;
        }
        if offset < s.start.saturating_add(s.removed) {
            out = out.saturating_sub(offset.saturating_sub(s.start));
            break;
        }
        out = out.saturating_sub(s.removed).saturating_add(s.inserted);
    }
    out
}

fn per_selected_line_edits(
    app: &mut App,
    id: DocumentId,
    build: impl Fn(usize, &Buffer) -> Option<Edit>,
) {
    let Some(doc) = app.doc(id) else { return };
    let cursors_before = doc.cursors.clone();
    let before = cursors_before.all().to_vec();

    let mut infos: Vec<(Edit, CursorId)> = Vec::new();
    let mut seen: HashSet<usize> = HashSet::new();
    for c in &before {
        let Some(doc) = app.doc(id) else { return };
        for line in selected_lines(c, &doc.buffer) {
            if !seen.insert(line) {
                continue;
            }
            if let Some(edit) = build(line, &doc.buffer) {
                infos.push((edit, c.id));
            }
        }
    }

    let mut shifts: Vec<LineShift> = infos
        .iter()
        .map(|(e, _)| LineShift {
            start: e.start,
            removed: e.end.saturating_sub(e.start),
            inserted: e.insert.len(),
        })
        .collect();
    shifts.sort_by_key(|s| s.start);

    let _ = apply_edit_batch_with_cursors(
        app,
        id,
        infos,
        &cursors_before,
        EditKind::Other,
        move |_, _| {
            before
                .iter()
                .map(|c| Cursor {
                    position: shift_through(c.position, &shifts),
                    anchor: shift_through(c.anchor, &shifts),
                    desired_col: 0,
                    id: c.id,
                })
                .collect()
        },
    );
}

pub fn indent(app: &mut App, id: DocumentId) {
    per_selected_line_edits(app, id, |line, buf| {
        let line_start = buf.line_start(line)?;
        Some(Edit {
            start: line_start,
            end: line_start,
            insert: "\t".to_string(),
        })
    });
}

pub fn outdent(app: &mut App, id: DocumentId) {
    per_selected_line_edits(app, id, dedent_edit_for_line);
}

fn dedent_edit_for_line(line: usize, buf: &Buffer) -> Option<Edit> {
    let line_start = buf.line_start(line)?;
    let line_end = buf.line_end(line)?;
    let line_text = buf.slice(line_start, line_end).unwrap_or("");

    let mut indent_end = 0usize;
    for ch in line_text.chars() {
        if ch == '\t' || ch == ' ' {
            indent_end += ch.len_utf8();
        } else {
            break;
        }
    }
    if indent_end == 0 {
        return None;
    }

    let mut remove = 1usize;
    if indent_end >= 4 {
        let space_run = line_text.bytes().take_while(|&b| b == b' ').count();
        if space_run >= 4 {
            remove = 4;
        }
    }
    if indent_end < remove {
        remove = indent_end;
    }

    Some(Edit {
        start: line_start,
        end: line_start + remove,
        insert: String::new(),
    })
}

/// Deletes the whole line under each (deduped) cursor: the whole
/// buffer when it's the only line; the line plus its own trailing `\n`
/// when a later line exists; otherwise (the last line) the PREVIOUS
/// line's trailing `\n` plus this line's own text, since the last line has
/// no trailing `\n` of its own to remove.
pub fn delete_line(app: &mut App, id: DocumentId) {
    per_line_edits(app, id, true, |line, buf| {
        let line_count = buf.line_count();
        if line_count == 1 {
            if buf.is_empty() {
                // An empty buffer's one line has nothing to delete — a
                // `0,0,""` edit would be a true no-op the user still has
                // to ⌘Z through (matching `outdent`'s own `None` on its
                // analogous no-op case, just below).
                return None;
            }
            return Some(Edit {
                start: 0,
                end: buf.len(),
                insert: String::new(),
            });
        }
        if line < line_count - 1 {
            Some(Edit {
                start: buf.line_start(line)?,
                end: buf.line_start(line + 1)?,
                insert: String::new(),
            })
        } else {
            Some(Edit {
                start: buf.line_end(line - 1)?,
                end: buf.line_end(line)?,
                insert: String::new(),
            })
        }
    });
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::commands::edit::undo;
    use crate::commands::test_support::selecting;
    use rune_core::cursor::{CursorSet, CursorSpec};
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
    fn indent_inserts_a_leading_tab() {
        let mut app = app_with("hello", 2);
        let id = app.active;
        indent(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "\thello");
    }

    #[test]
    fn tab_indents_every_line_the_selection_touches() {
        let mut app = app_with("a\nb\nc", 0);
        let id = app.active;
        selecting(&mut app, id, 0, 5);
        indent(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "\ta\n\tb\n\tc");
    }

    #[test]
    fn a_selection_ending_at_column_zero_does_not_indent_that_line() {
        let mut app = app_with("a\nb\nc", 0);
        let id = app.active;
        selecting(&mut app, id, 0, 4);
        indent(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "\ta\n\tb\nc");
    }

    #[test]
    fn the_selection_still_covers_the_same_lines_after_indent() {
        let mut app = app_with("a\nb", 0);
        let id = app.active;
        selecting(&mut app, id, 0, 3);
        indent(&mut app, id);
        let primary = app.doc(id).unwrap().cursors.primary();
        assert_eq!((primary.anchor, primary.position), (0, 5));
    }

    #[test]
    fn indent_keeps_the_caret_column_when_there_is_no_selection() {
        let mut app = app_with("hello", 2);
        let id = app.active;
        indent(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "\thello");
        assert_eq!(app.doc(id).unwrap().cursors.primary().position, 3);
    }

    #[test]
    fn outdent_dedents_every_selected_line() {
        let mut app = app_with("\ta\n\tb", 0);
        let id = app.active;
        selecting(&mut app, id, 0, 5);
        outdent(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "a\nb");
    }

    #[test]
    fn outdent_puts_a_caret_inside_removed_indentation_at_the_line_start() {
        let mut app = app_with("\thello", 1);
        let id = app.active;
        outdent(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "hello");
        assert_eq!(app.doc(id).unwrap().cursors.primary().position, 0);
    }

    #[test]
    fn outdent_over_a_selection_with_no_indentation_is_a_no_op() {
        let mut app = app_with("a\nb", 0);
        let id = app.active;
        selecting(&mut app, id, 0, 3);
        outdent(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "a\nb");
        assert_eq!(app.doc(id).unwrap().journal.len(), 0);
        let primary = app.doc(id).unwrap().cursors.primary();
        assert_eq!((primary.anchor, primary.position), (0, 3));
    }

    #[test]
    fn indenting_a_selection_is_one_undo_step() {
        let mut app = app_with("a\nb\nc", 0);
        let id = app.active;
        selecting(&mut app, id, 0, 5);
        let steps_before = app.doc(id).unwrap().journal.len();
        indent(&mut app, id);
        assert_eq!(app.doc(id).unwrap().journal.len(), steps_before + 1);
        undo(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "a\nb\nc");
    }

    #[test]
    fn two_cursors_on_one_line_still_indent_it_once() {
        let mut app = app_with("ab", 0);
        let id = app.active;
        let doc = app.doc_mut(id).unwrap();
        doc.cursors = doc.cursors.clone().add(CursorSpec {
            position: 2,
            anchor: 2,
            desired_col: 0,
        });
        indent(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "\tab");
    }

    #[test]
    fn outdent_removes_one_leading_tab() {
        let mut app = app_with("\thello", 3);
        let id = app.active;
        outdent(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "hello");
    }

    #[test]
    fn outdent_removes_up_to_four_leading_spaces() {
        let mut app = app_with("    hello", 5);
        let id = app.active;
        outdent(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "hello");
    }

    #[test]
    fn outdent_on_a_line_with_no_indentation_is_a_no_op() {
        let mut app = app_with("hello", 0);
        let id = app.active;
        outdent(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "hello");
        assert_eq!(app.doc(id).unwrap().journal.len(), 0);
    }

    #[test]
    fn delete_line_removes_the_middle_line_and_its_own_newline() {
        let mut app = app_with("one\ntwo\nthree", "one\n".len());
        let id = app.active;
        delete_line(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "one\nthree");
    }

    #[test]
    fn delete_line_on_the_last_line_absorbs_the_previous_newline() {
        let mut app = app_with("one\ntwo", "one\n".len());
        let id = app.active;
        delete_line(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "one");
    }

    #[test]
    fn delete_line_on_a_single_line_buffer_clears_it() {
        let mut app = app_with("only", 2);
        let id = app.active;
        delete_line(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "");
    }

    #[test]
    fn delete_line_on_an_empty_buffer_is_a_true_no_op_not_a_journaled_step() {
        let mut app = app_with("", 0);
        let id = app.active;
        delete_line(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "");
        assert_eq!(
            app.doc(id).unwrap().journal.len(),
            0,
            "an empty buffer has nothing to delete — no Step, nothing to ⌘Z through"
        );
    }

    #[test]
    fn delete_line_then_undo_restores_the_buffer() {
        let mut app = app_with("one\ntwo\nthree", "one\n".len());
        let id = app.active;
        let original = app.doc(id).unwrap().buffer.content().to_string();
        delete_line(&mut app, id);
        undo(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), original);
    }
}
