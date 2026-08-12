//! Multi-cursor management (plan WP9.S3): add-cursor-above/add-cursor-below.
//! `multicursor.escape`, the third command in this family, lives in
//! `commands::nav::escape`.
//!
//! Doc-local (plan WP1 decision 4), like `commands::nav`: pure cursor
//! placement, no buffer mutation, so this takes `&mut Document` directly
//! rather than `(app, id)` and never touches `apply_edit_batch_with_
//! cursors`/`commit_edit_batch`.
//!
//! Deliberately NOT built on `nav::move_row` (the wrap-aware vertical
//! motion arrow-up/down uses): add-cursor-above/below works in
//! plain BUFFER-line space (`OffsetToLineCol`/`LineStart`/`LineEnd`), not
//! wrap space — it targets the next LOGICAL line,
//! ignoring soft-wrap entirely, unlike arrow-key vertical motion. Building
//! it onto `move_row` would silently make it wrap-aware, which this plan
//! did not ask for.

use rune_core::buffer::Buffer;
use rune_core::coords::{BufferPoint, VisualCol, WrapPoint};
use rune_core::cursor::CursorSpec;
use rune_md::element::doc::ViewSnapshots;

use crate::document::Document;

/// Shared direction driver: `dir < 0` adds a cursor on the line above the
/// TOPMOST existing cursor; `dir > 0` adds one on the line below the
/// BOTTOMMOST.
fn add_cursor(doc: &mut Document, dir: isize) {
    let all = doc.cursors.all();
    let Some(&first) = all.first() else { return };

    // Find the extreme cursor to add adjacent to: topmost for dir<0,
    // bottommost for dir>0. Ties keep the earlier cursor in iteration
    // order.
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

    // `desired_col` is a terminal-CELL count, never a byte column —
    // it is measured on ONE line and only ever meaningful when replayed
    // through the same cell->byte conversion `commands::nav_scroll::
    // move_row` uses (`byte_col_from_visual`), never as a raw `BufferPoint.
    // col`: a cell count replayed as bytes lands mid-character the moment
    // the target line's bytes-per-cell ratio differs from the source
    // line's (e.g. CJK on one line, ASCII on the other).
    let view = doc.view();
    let desired = if extreme.desired_col == 0 {
        cell_col_at(&view, &doc.buffer, bp)
    } else {
        VisualCol(extreme.desired_col)
    };
    let new_bp = visual_col_on_line(&view, &doc.buffer, target_line, desired);
    let new_offset = doc.buffer.line_col_to_offset(new_bp);

    let new_cursor = CursorSpec {
        position: new_offset,
        anchor: new_offset,
        desired_col: desired.0,
    };

    doc.cursors = doc.cursors.add(new_cursor);
}

/// Converts a full buffer point to its terminal-CELL visual column — the
/// same buffer->syntax->wrap walk `commands::nav::update_horizontal` uses
/// when a horizontal motion recomputes `desired_col` from a landed
/// position. Used here only as the `desired_col == 0` sentinel's fallback
/// (a cursor that has never had a real `desired_col` established yet), so
/// that fallback carries the same unit as every other `desired_col`
/// instead of smuggling a byte column in under the same field.
fn cell_col_at(view: &ViewSnapshots, buf: &Buffer, bp: BufferPoint) -> VisualCol {
    let sp = view.syntax.buffer_to_syntax(bp);
    let wp = view.wrap.syntax_to_wrap(sp);
    VisualCol(view.wrap.visual_col(buf.content(), wp.row, wp.col))
}

/// Places a terminal-CELL visual column onto a DIFFERENT logical line's own
/// first wrap row — the exact `byte_col_from_visual` conversion
/// `commands::nav_scroll::move_row` performs for vertical motion, reused
/// here so add-cursor-above/below (which targets the next LOGICAL line,
/// ignoring soft-wrap — see this module's doc comment) still converts the
/// cell column correctly rather than replaying it as a byte offset.
fn visual_col_on_line(
    view: &ViewSnapshots,
    buf: &Buffer,
    line: usize,
    desired: VisualCol,
) -> BufferPoint {
    let line_start_sp = view.syntax.buffer_to_syntax(BufferPoint { line, col: 0 });
    let row = view.wrap.syntax_to_wrap(line_start_sp).row;
    let byte_col = view
        .wrap
        .byte_col_from_visual(buf.content(), row, desired.0);
    let sp = view
        .wrap
        .wrap_to_syntax(buf.content(), WrapPoint { row, col: byte_col });
    view.syntax.syntax_to_buffer(sp)
}

/// Adds a cursor on the logical line above the topmost existing cursor, at
/// its desired visual column.
pub fn add_cursor_above(doc: &mut Document) {
    add_cursor(doc, -1);
}

/// Adds a cursor on the logical line below the bottommost existing cursor,
/// at its desired visual column.
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
    fn add_cursor_below_converts_desired_col_from_cells_not_raw_bytes() {
        // line1 packs two double-width CJK glyphs before six single-byte
        // ASCII characters, so a CELL column and the SAME NUMBER used as a
        // raw BYTE column land on different characters — the exact defect
        // class this guards: `desired_col` is always a cell count, and
        // feeding it straight into a byte-column buffer API silently lands
        // the cursor mid-character (or on the wrong character entirely).
        let mut doc = doc_with("x\n日本CDEFGH", 0);
        // 5 CELLS: "日"(2) + "本"(2) + "C"(1).
        doc.cursors = CursorSet::new_from_specs(&[CursorSpec {
            position: 0,
            anchor: 0,
            desired_col: 5,
        }]);

        add_cursor_below(&mut doc);

        assert_eq!(doc.cursors.len(), 2);
        let new_cursor = doc.cursors.all()[1];
        let line1_start = doc.buffer.line_start(1).expect("line 1 exists");
        // Correct: cell column 5 on "日本CDEFGH" lands right after "日本C"
        // (2+2+1 = 5 cells), byte offset 7 (3+3+1) into that line, on 'D'.
        // Treating `5` as a raw byte column instead (the pre-fix behavior)
        // lands inside "本" (bytes 3..6) and snaps down to its start (byte
        // 3) — a different, wrong character.
        assert_eq!(new_cursor.position, line1_start + 7);
    }

    #[test]
    fn add_cursor_below_targets_the_bottommost_of_multiple_cursors() {
        // Cursors on line 0 and line 1; the new one must land on line 2
        // (adjacent to the BOTTOMMOST existing cursor), not line 1
        // (adjacent to the topmost).
        let mut doc = doc_with("aaa\nbbb\nccc\nddd", 0);
        doc.cursors = doc.cursors.add(CursorSpec {
            position: "aaa\n".len(),
            anchor: "aaa\n".len(),
            desired_col: 0,
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
