//! Multi-cursor management (plan WP9.S3). Port of
//! `pkg/ui/components/textedit/commands_multi.go:execMulticursorAdd`/
//! `execMulticursorAddAbove`/`execMulticursorAddBelow`
//! (`multicursor.escape`, the third command in that Go file, is already
//! ported as `commands::nav::escape`).
//!
//! Doc-local (plan WP1 decision 4), like `commands::nav`: pure cursor
//! placement, no buffer mutation, so this takes `&mut Document` directly
//! rather than `(app, id)` and never touches `apply_edit_batch_with_
//! cursors`/`commit_edit_batch`.
//!
//! Deliberately NOT built on `nav::move_row` (the wrap-aware vertical
//! motion arrow-up/down uses): Go's own `execMulticursorAdd` works in
//! plain BUFFER-line space (`OffsetToLineCol`/`LineStart`/`LineEnd`), not
//! wrap space — add-cursor-above/below targets the next LOGICAL line,
//! ignoring soft-wrap entirely, unlike arrow-key vertical motion. Porting
//! it onto `move_row` would silently make it wrap-aware, a behavior Go
//! does not have and this plan did not ask for.

use rune_core::coords::BufferPoint;
use rune_core::cursor::Cursor;

use crate::document::Document;

/// Shared direction driver: `dir < 0` adds a cursor on the line above the
/// TOPMOST existing cursor; `dir > 0` adds one on the line below the
/// BOTTOMMOST. Port of `commands_multi.go:execMulticursorAdd`.
fn add_cursor(doc: &mut Document, dir: isize) {
    let all = doc.cursors.all();
    let Some(&first) = all.first() else { return };

    // Find the extreme cursor to add adjacent to: topmost for dir<0,
    // bottommost for dir>0. Ties keep the earlier cursor in iteration
    // order, matching Go's strict `<`/`>` comparison.
    let mut extreme = first;
    for &c in all.iter().skip(1) {
        if (dir < 0 && c.position < extreme.position) || (dir > 0 && c.position > extreme.position)
        {
            extreme = c;
        }
    }

    let bp: BufferPoint = doc.buffer.offset_to_line_col(extreme.position);
    if dir < 0 && bp.line == 0 {
        return;
    }
    let line_count = doc.buffer.line_count();
    if dir > 0 && (line_count == 0 || bp.line >= line_count - 1) {
        return;
    }

    let target_line = if dir < 0 { bp.line - 1 } else { bp.line + 1 };
    let desired_col = if extreme.desired_col == 0 {
        bp.col
    } else {
        extreme.desired_col
    };

    let line_len = doc.buffer.line_end(target_line) - doc.buffer.line_start(target_line);
    let col = desired_col.min(line_len);

    let new_offset = doc.buffer.line_start(target_line) + col;
    let new_cursor = Cursor {
        position: new_offset,
        anchor: new_offset,
        desired_col,
        id: 0,
    };

    doc.cursors = doc.cursors.add(new_cursor);
}

/// Port of `commands_multi.go:execMulticursorAddAbove`.
pub fn add_cursor_above(doc: &mut Document) {
    add_cursor(doc, -1);
}

/// Port of `commands_multi.go:execMulticursorAddBelow`.
pub fn add_cursor_below(doc: &mut Document) {
    add_cursor(doc, 1);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use rune_core::buffer::Buffer;
    use rune_core::cursor::CursorSet;

    fn doc_with(content: &str, cursor_offset: usize) -> Document {
        let mut doc = Document::new(Buffer::new(content));
        doc.cursors = CursorSet::new(cursor_offset.min(content.len()));
        doc.viewport.set_size(80, 23);
        doc
    }

    #[test]
    fn add_cursor_below_adds_one_on_the_next_line_same_column() {
        let mut doc = doc_with("one\ntwo\nthree", 1); // col 1 of line 0
        add_cursor_below(&mut doc);
        assert_eq!(doc.cursors.len(), 2);
        let all = doc.cursors.all();
        assert_eq!(doc.buffer.offset_to_line_col(all[1].position).line, 1);
        assert_eq!(doc.buffer.offset_to_line_col(all[1].position).col, 1);
    }

    #[test]
    fn add_cursor_above_adds_one_on_the_previous_line_same_column() {
        let mut doc = doc_with("one\ntwo\nthree", "one\ntwo\nthr".len());
        add_cursor_above(&mut doc);
        assert_eq!(doc.cursors.len(), 2);
        let all = doc.cursors.all();
        assert_eq!(doc.buffer.offset_to_line_col(all[0].position).line, 1);
    }

    #[test]
    fn add_cursor_above_clamps_to_a_shorter_line() {
        let mut doc = doc_with("ab\nlonger line", "ab\nlonger".len());
        add_cursor_above(&mut doc);
        let all = doc.cursors.all();
        // Line 0 ("ab") is only 2 bytes long — the new cursor clamps to it.
        assert_eq!(all[0].position, 2);
    }

    #[test]
    fn add_cursor_above_at_the_first_line_is_a_no_op() {
        let mut doc = doc_with("only", 1);
        add_cursor_above(&mut doc);
        assert_eq!(doc.cursors.len(), 1);
    }

    #[test]
    fn add_cursor_below_at_the_last_line_is_a_no_op() {
        let mut doc = doc_with("one\ntwo", "one\n".len() + 1);
        add_cursor_below(&mut doc);
        assert_eq!(doc.cursors.len(), 1);
    }

    #[test]
    fn add_cursor_below_targets_the_bottommost_of_multiple_cursors() {
        // Cursors on line 0 and line 1; the new one must land on line 2
        // (adjacent to the BOTTOMMOST existing cursor), not line 1
        // (adjacent to the topmost).
        let mut doc = doc_with("aaa\nbbb\nccc\nddd", 0);
        doc.cursors = doc.cursors.add(Cursor {
            position: "aaa\n".len(),
            anchor: "aaa\n".len(),
            desired_col: 0,
            id: 0,
        });
        assert_eq!(doc.cursors.len(), 2, "fixture must hold two cursors");
        add_cursor_below(&mut doc);
        assert_eq!(doc.cursors.len(), 3);
        let lines: Vec<usize> = doc
            .cursors
            .all()
            .iter()
            .map(|c| doc.buffer.offset_to_line_col(c.position).line)
            .collect();
        assert_eq!(lines, vec![0, 1, 2]);
    }
}
