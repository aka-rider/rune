//! Line duplication and reordering commands: split out of the sibling
//! `edit_lines` module (that module was already over the 500-line
//! budget). Implements clone-line-up/down and move-line-up/down commands.
//! `clone_line_up`/`clone_line_down` reuse `edit_lines`'s
//! `per_line_edits` (via `pub(crate)`); `move_line_up`/`move_line_down`
//! hand-place the resulting cursor at a column instead of the edit's end,
//! so those two call `edit_core::apply_edit_batch_with_cursors` directly.

use rune_core::buffer::{Buffer, Edit};
use rune_core::coords::BufferOffset;
use rune_core::cursor::Cursor;
use rune_core::undo::EditKind;

use crate::app::App;
use crate::commands::edit_core::apply_edit_batch_with_cursors;
use crate::commands::edit_lines::per_line_edits;
use crate::document::DocumentId;

struct LineParts {
    text: String,
    terminator: String,
}

fn line_parts(buf: &Buffer, n: usize) -> Option<LineParts> {
    let start = buf.line_start(n)?;
    let content_end = buf.line_content_end(n)?;
    let term = buf.line_terminator_range(n)?;
    Some(LineParts {
        text: buf.slice(start, content_end)?.to_string(),
        terminator: buf.slice(term.start, term.end)?.to_string(),
    })
}

/// Inserts a copy of each (non-deduped) cursor's line directly above it.
/// `line == 0` skips that cursor (no line to clone above).
pub fn clone_line_up(app: &mut App, id: DocumentId) {
    per_line_edits(app, id, false, |line, buf| {
        if line == 0 {
            return None;
        }
        let line_start = buf.line_start(line)?;
        let parts = line_parts(buf, line)?;
        let terminator = if parts.terminator.is_empty() {
            "\n"
        } else {
            parts.terminator.as_str()
        };
        Some(Edit {
            start: line_start,
            end: line_start,
            insert: format!("{}{terminator}", parts.text),
        })
    });
}

/// Inserts a copy of each (non-deduped) cursor's line directly below it.
pub fn clone_line_down(app: &mut App, id: DocumentId) {
    per_line_edits(app, id, false, |line, buf| {
        let content_end = buf.line_content_end(line)?;
        let parts = line_parts(buf, line)?;
        let terminator = if parts.terminator.is_empty() {
            "\n"
        } else {
            parts.terminator.as_str()
        };
        Some(Edit {
            start: content_end,
            end: content_end,
            insert: format!("{terminator}{}", parts.text),
        })
    });
}

/// Swaps the FIRST cursor's line with the one directly above it, in a
/// single edit, and collapses the whole cursor set to just that one
/// cursor: move-line only ever acts on the first
/// cursor, dropping any others in the set. The surviving cursor lands at
/// the same COLUMN it held within its line, now inside the moved block —
/// not at the edit's end, which is why this calls `apply_edit_batch_with_
/// cursors` directly instead of the generic `commit_edit_batch`.
pub fn move_line_up(app: &mut App, id: DocumentId) {
    let Some(doc) = app.doc(id) else { return };
    let cursors_before = doc.cursors.clone();
    let Some(c) = cursors_before.all().first() else {
        return;
    };
    let bp = doc.buffer.offset_to_line_col(c.position.get());
    if bp.line == 0 {
        return;
    }
    let l = bp.line;
    let Some(prev_start) = doc.buffer.line_start(l - 1) else {
        return;
    };
    let Some(prev_parts) = line_parts(&doc.buffer, l - 1) else {
        return;
    };
    let Some(line_start) = doc.buffer.line_start(l) else {
        return;
    };
    let Some(cur_parts) = line_parts(&doc.buffer, l) else {
        return;
    };
    let Some(edit_end) = doc.buffer.line_terminator_range(l).map(|r| r.end) else {
        return;
    };

    let separator = prev_parts.terminator;
    let trailing = cur_parts.terminator;

    let cid = c.id;
    let desired_col = c.desired_col;
    let col = (c.position.get() - line_start).min(cur_parts.text.len());
    let new_pos = BufferOffset(prev_start + col);
    let edit = Edit {
        start: prev_start,
        end: edit_end,
        insert: format!("{}{separator}{}{trailing}", cur_parts.text, prev_parts.text),
    };

    let _ = apply_edit_batch_with_cursors(
        app,
        id,
        vec![(edit, cid)],
        &cursors_before,
        EditKind::Other,
        move |_, _| {
            vec![Cursor {
                position: new_pos,
                anchor: new_pos,
                desired_col,
                id: cid,
            }]
        },
    );
}

/// Mirror of `move_line_up` above.
pub fn move_line_down(app: &mut App, id: DocumentId) {
    let Some(doc) = app.doc(id) else { return };
    let cursors_before = doc.cursors.clone();
    let Some(c) = cursors_before.all().first() else {
        return;
    };
    let bp = doc.buffer.offset_to_line_col(c.position.get());
    let line_count = doc.buffer.line_count();
    if line_count == 0 || bp.line >= line_count - 1 {
        return;
    }
    let l = bp.line;
    let Some(line_start) = doc.buffer.line_start(l) else {
        return;
    };
    let Some(cur_parts) = line_parts(&doc.buffer, l) else {
        return;
    };
    let Some(next_parts) = line_parts(&doc.buffer, l + 1) else {
        return;
    };
    let Some(edit_end) = doc.buffer.line_terminator_range(l + 1).map(|r| r.end) else {
        return;
    };

    let separator = cur_parts.terminator;
    let trailing = next_parts.terminator;

    let cid = c.id;
    let desired_col = c.desired_col;
    let col = (c.position.get() - line_start).min(cur_parts.text.len());
    let new_pos = BufferOffset(line_start + next_parts.text.len() + separator.len() + col);
    let edit = Edit {
        start: line_start,
        end: edit_end,
        insert: format!("{}{separator}{}{trailing}", next_parts.text, cur_parts.text),
    };

    let _ = apply_edit_batch_with_cursors(
        app,
        id,
        vec![(edit, cid)],
        &cursors_before,
        EditKind::Other,
        move |_, _| {
            vec![Cursor {
                position: new_pos,
                anchor: new_pos,
                desired_col,
                id: cid,
            }]
        },
    );
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::commands::edit::undo;
    use crate::commands::edit_lines::{delete_line, indent, outdent};
    use rune_core::buffer::Buffer;
    use rune_core::coords::VisualCol;
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
    fn clone_line_up_inserts_a_copy_above() {
        let mut app = app_with("one\ntwo", "one\n".len() + 1);
        let id = app.active;
        clone_line_up(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "one\ntwo\ntwo");
    }

    #[test]
    fn clone_line_up_at_the_first_line_is_a_no_op() {
        let mut app = app_with("only", 1);
        let id = app.active;
        clone_line_up(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "only");
        assert_eq!(app.doc(id).unwrap().journal.len(), 0);
    }

    #[test]
    fn clone_line_down_inserts_a_copy_below() {
        let mut app = app_with("one\ntwo", 1);
        let id = app.active;
        clone_line_down(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "one\none\ntwo");
    }

    #[test]
    fn clone_line_down_then_undo_restores_the_buffer() {
        let mut app = app_with("one\ntwo", 1);
        let id = app.active;
        let original = app.doc(id).unwrap().buffer.content().to_string();
        clone_line_down(&mut app, id);
        undo(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), original);
    }

    #[test]
    fn clone_line_down_with_two_cursors_on_the_same_line_clones_it_twice_and_keeps_both_cursors() {
        // Two cursors sharing "two" derive byte-identical zero-width
        // insert edits at the same point (`per_line_edits(dedupe=false)`
        // keys on `line_start`, not on cursor identity) — the exact shape
        // `edit_core::coalesce_touching_edits` used to wrongly collapse
        // 2->1, silently dropping one cursor's own clone. Both edits must
        // survive uncoalesced: the line clones once PER CURSOR, and the
        // cursor set stays at 2.
        let mut app = app_with("one\ntwo", 4);
        let id = app.active;
        let doc = app.doc_mut(id).unwrap();
        doc.cursors = doc.cursors.clone().add(CursorSpec {
            position: BufferOffset(7),
            anchor: BufferOffset(7),
            desired_col: VisualCol(0),
        });
        assert_eq!(
            doc.cursors.len(),
            2,
            "fixture must hold two cursors on the same line"
        );

        clone_line_down(&mut app, id);

        assert_eq!(app.doc(id).unwrap().buffer.content(), "one\ntwo\ntwo\ntwo");
        assert_eq!(
            app.doc(id).unwrap().cursors.len(),
            2,
            "neither cursor's clone may be silently dropped"
        );
    }

    #[test]
    fn move_line_up_swaps_with_the_previous_line_preserving_column() {
        let mut app = app_with("one\ntwo\nthree", "one\ntw".len());
        let id = app.active;
        move_line_up(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "two\none\nthree");
        // Column within "two" (2 bytes in) is preserved on the moved line.
        assert_eq!(
            app.doc(id).unwrap().cursors.primary().position,
            BufferOffset(2)
        );
    }

    #[test]
    fn move_line_up_at_the_first_line_is_a_no_op() {
        let mut app = app_with("one\ntwo", 1);
        let id = app.active;
        move_line_up(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "one\ntwo");
        assert_eq!(app.doc(id).unwrap().journal.len(), 0);
    }

    #[test]
    fn move_line_down_then_undo_restores_the_buffer_byte_for_byte() {
        let mut app = app_with("one\ntwo\nthree", 1);
        let id = app.active;
        let original = app.doc(id).unwrap().buffer.content().to_string();
        move_line_down(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "two\none\nthree");
        undo(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), original);
    }

    #[test]
    fn move_line_down_at_the_last_line_is_a_no_op() {
        let mut app = app_with("one\ntwo", "one\n".len() + 1);
        let id = app.active;
        move_line_down(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "one\ntwo");
        assert_eq!(app.doc(id).unwrap().journal.len(), 0);
    }

    /// Every line command in this module funnels through the SAME
    /// `apply_edit_batch_with_cursors` chokepoint `edit.rs`'s own commands
    /// do (F1) — the other half of `edit::tests::
    /// read_only_blocks_typing_backspace_and_newline`'s regression.
    #[test]
    fn read_only_blocks_line_commands() {
        let mut app = app_with("one\ntwo\nthree", "one\n".len());
        let id = app.active;
        app.doc_mut(id).unwrap().read_only = crate::document::ReadOnly::Always;
        let before = app.doc(id).unwrap().buffer.content().to_string();

        indent(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), before, "indent");
        outdent(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), before, "outdent");
        delete_line(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), before, "delete-line");
        clone_line_up(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), before, "clone-up");
        clone_line_down(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), before, "clone-down");
        move_line_up(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), before, "move-up");
        move_line_down(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), before, "move-down");
        assert_eq!(app.doc(id).unwrap().journal.len(), 0);
    }

    #[test]
    fn moving_the_last_line_of_a_crlf_file_keeps_its_terminator() {
        let mut app = app_with("A\r\nB", "A\r\n".len());
        let id = app.active;
        move_line_up(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "B\r\nA");
    }

    #[test]
    fn moving_a_line_below_the_crlf_files_last_unterminated_line_keeps_it_unterminated() {
        let mut app = app_with("A\r\nB", 0);
        let id = app.active;
        move_line_down(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "B\r\nA");
    }

    #[test]
    fn move_line_up_preserves_crlf_terminators_between_two_crlf_lines() {
        let mut app = app_with("one\r\ntwo\r\nthree", "one\r\ntw".len());
        let id = app.active;
        move_line_up(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "two\r\none\r\nthree");
    }

    #[test]
    fn clone_line_up_of_the_crlf_files_last_unterminated_line_terminates_the_copy() {
        let mut app = app_with("A\r\nB", "A\r\n".len());
        let id = app.active;
        clone_line_up(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "A\r\nB\nB");
    }

    #[test]
    fn clone_line_down_of_the_crlf_files_last_unterminated_line_terminates_the_original() {
        let mut app = app_with("A\r\nB", "A\r\n".len());
        let id = app.active;
        clone_line_down(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "A\r\nB\nB");
    }

    #[test]
    fn clone_line_down_preserves_a_crlf_terminator_verbatim() {
        let mut app = app_with("A\r\nB\r\nC", 0);
        let id = app.active;
        clone_line_down(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "A\r\nA\r\nB\r\nC");
    }

    #[test]
    fn a_lone_cr_file_has_a_single_line_so_move_up_and_down_are_both_no_ops() {
        let mut app = app_with("A\rB", 1);
        let id = app.active;
        assert_eq!(app.doc(id).unwrap().buffer.line_count(), 1);
        move_line_up(&mut app, id);
        move_line_down(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "A\rB");
    }

    #[test]
    fn clone_line_down_on_a_lone_cr_file_duplicates_the_cr_verbatim() {
        let mut app = app_with("A\rB", 1);
        let id = app.active;
        clone_line_down(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "A\rB\nA\rB");
    }
}
