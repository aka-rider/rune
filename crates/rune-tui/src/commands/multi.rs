use rune_core::buffer::Buffer;
use rune_core::coords::{BufferOffset, BufferPoint, VisualCol, WrapPoint};
use rune_core::cursor::CursorSpec;
use rune_md::element::doc::ViewSnapshots;

use crate::document::Document;

/// Targets the next LOGICAL line in plain buffer-line space, ignoring
/// soft-wrap entirely — deliberately not built on `nav::move_row` (the
/// wrap-aware vertical motion arrow-up/down uses), which would silently
/// make this wrap-aware too.
fn add_cursor(doc: &mut Document, dir: isize) {
    let all = doc.cursors.all();
    let Some(&first) = all.first() else { return };

    let mut extreme = first;
    for &c in all.iter().skip(1) {
        if (dir < 0 && c.position < extreme.position) || (dir > 0 && c.position > extreme.position)
        {
            extreme = c;
        }
    }

    let bp: BufferPoint = doc.buffer.offset_to_line_col(extreme.position.get());
    if dir < 0 && bp.line == 0 {
        return;
    }
    let line_count = doc.buffer.line_count();
    if dir > 0 && (line_count == 0 || bp.line >= line_count - 1) {
        return;
    }

    let target_line = if dir < 0 { bp.line - 1 } else { bp.line + 1 };

    let view = doc.view();
    let desired = if extreme.desired_col == VisualCol(0) {
        cell_col_at(&view, &doc.buffer, bp)
    } else {
        extreme.desired_col
    };
    let new_bp = visual_col_on_line(&view, &doc.buffer, target_line, desired);
    let new_offset = doc.buffer.line_col_to_offset(new_bp);

    let new_cursor = CursorSpec {
        position: BufferOffset(new_offset),
        anchor: BufferOffset(new_offset),
        desired_col: desired,
    };

    doc.cursors = doc.cursors.add(new_cursor);
}

fn cell_col_at(view: &ViewSnapshots, buf: &Buffer, bp: BufferPoint) -> VisualCol {
    let sp = view.syntax.buffer_to_syntax(bp);
    let wp = view.wrap.syntax_to_wrap(sp);
    VisualCol(view.wrap.visual_col(buf.content(), wp.row, wp.col))
}

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

pub fn add_cursor_above(doc: &mut Document) {
    add_cursor(doc, -1);
}

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
        let mut doc = doc_with("one\ntwo\nthree", 1);
        add_cursor_below(&mut doc);
        assert_eq!(doc.cursors.len(), 2);
        let all = doc.cursors.all();
        assert_eq!(doc.buffer.offset_to_line_col(all[1].position.get()).line, 1);
        assert_eq!(doc.buffer.offset_to_line_col(all[1].position.get()).col, 1);
    }

    #[test]
    fn add_cursor_above_adds_one_on_the_previous_line_same_column() {
        let mut doc = doc_with("one\ntwo\nthree", "one\ntwo\nthr".len());
        add_cursor_above(&mut doc);
        assert_eq!(doc.cursors.len(), 2);
        let all = doc.cursors.all();
        assert_eq!(doc.buffer.offset_to_line_col(all[0].position.get()).line, 1);
    }

    #[test]
    fn add_cursor_above_clamps_to_a_shorter_line() {
        let mut doc = doc_with("ab\nlonger line", "ab\nlonger".len());
        add_cursor_above(&mut doc);
        let all = doc.cursors.all();
        assert_eq!(all[0].position, BufferOffset(2));
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
        let mut doc = doc_with("x\n日本CDEFGH", 0);
        doc.cursors = CursorSet::new_from_specs(&[CursorSpec {
            position: BufferOffset(0),
            anchor: BufferOffset(0),
            desired_col: VisualCol(5),
        }]);

        add_cursor_below(&mut doc);

        assert_eq!(doc.cursors.len(), 2);
        let new_cursor = doc.cursors.all()[1];
        let line1_start = doc.buffer.line_start(1).expect("line 1 exists");
        // Cell column 5 on "日本CDEFGH" is "日"+"本"+"C" (2+2+1 cells),
        // byte offset 7 (3+3+1) into the line, landing on 'D'.
        assert_eq!(new_cursor.position, BufferOffset(line1_start + 7));
    }

    #[test]
    fn add_cursor_below_targets_the_bottommost_of_multiple_cursors() {
        let mut doc = doc_with("aaa\nbbb\nccc\nddd", 0);
        doc.cursors = doc.cursors.add(CursorSpec {
            position: BufferOffset("aaa\n".len()),
            anchor: BufferOffset("aaa\n".len()),
            desired_col: VisualCol(0),
        });
        assert_eq!(doc.cursors.len(), 2, "fixture must hold two cursors");
        add_cursor_below(&mut doc);
        assert_eq!(doc.cursors.len(), 3);
        let lines: Vec<usize> = doc
            .cursors
            .all()
            .iter()
            .map(|c| doc.buffer.offset_to_line_col(c.position.get()).line)
            .collect();
        assert_eq!(lines, vec![0, 1, 2]);
    }
}
