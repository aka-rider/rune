//! Line-oriented editing commands (plan WP9.S6 §1.6 split out of
//! `edit.rs`, which was 802 lines before WP9 added anything). Ports Go's
//! indent/outdent and delete-line commands (plan WP9.S2). Line duplication
//! and reordering (clone-line-up/down, move-line-up/down) live in the
//! sibling `edit_lines_move` module (§1.6: this file was already over the
//! 500-line budget); that module reaches back into this one for the shared
//! `per_line_edits` driver.
//!
//! `per_line_edits` below (indent/outdent/delete-line/clone-line) shares
//! `edit_core::commit_edit_batch`'s generic "each surviving cursor lands
//! at its own edit's `AppliedEdit::end`" rule — Go's `computePostEditCursors`
//! formula (`newPos = edit.Start + shift + insLen`) reduces to exactly
//! that rule for every one of these commands, since `AppliedEdit::end` is
//! `start + insert.len()` in the same post-shift coordinates
//! `Buffer::apply_edits` already produces. Only `move_line_up`/`down` is a
//! genuine exception: Go's own `execMoveLineUp`/`execMoveLineDown` are NOT
//! built on `buildEditResultFromInfos` at all — they hand-place the single
//! resulting cursor at a COLUMN within the moved line, not at the edit's
//! end — so those two call `edit_core::apply_edit_batch_with_cursors`
//! directly with that custom rule instead of going through
//! `commit_edit_batch`.

use std::collections::HashSet;

use rune_core::buffer::{Buffer, Edit};

use crate::app::App;
use crate::commands::edit_core::commit_edit_batch;
use crate::document::DocumentId;

/// Port of `commands_edit_lines.go:perLineEdits`. `dedupe=true` (indent,
/// outdent, delete-line) skips a line an earlier cursor in this same batch
/// already produced an edit for — two cursors on one line must not
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

    let mut infos: Vec<(Edit, u32)> = Vec::new();
    let mut seen: HashSet<usize> = HashSet::new();
    for c in &all {
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

    commit_edit_batch(app, id, infos, cursors_before);
}

/// Port of `commands_edit_lines_indent.go:execIndentLine` (Tab).
pub fn indent(app: &mut App, id: DocumentId) {
    per_line_edits(app, id, true, |line, buf| {
        let line_start = buf.line_start(line)?;
        Some(Edit {
            start: line_start,
            end: line_start,
            insert: "\t".to_string(),
        })
    });
}

/// Port of `commands_edit_lines_indent.go:execDedentLine` (Shift+Tab):
/// removes up to one leading tab, or up to 4 leading spaces if the line
/// starts with at least 4 of them.
pub fn outdent(app: &mut App, id: DocumentId) {
    per_line_edits(app, id, true, dedent_edit_for_line);
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

/// Port of `commands_edit_lines_multi.go:execDeleteLineMulti` (plan
/// WP9.S2). Deletes the whole line under each (deduped) cursor: the whole
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

    #[test]
    fn indent_inserts_a_leading_tab() {
        let mut app = app_with("hello", 2);
        let id = app.active;
        indent(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "\thello");
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
