use rune_core::coords::{BufferOffset, DisplayRow, WrapPoint, WrapRow};
use rune_core::cursor::{Cursor, CursorSet};
use rune_md::element::doc::ViewSnapshots;

use crate::document::Document;
use crate::keymap::Extend;
use crate::viewport::ScrollMode;

fn wrap_row_at(view: &ViewSnapshots, buf: &rune_core::buffer::Buffer, position: usize) -> usize {
    let bp = buf.offset_to_line_col(position);
    let sp = view.syntax.buffer_to_syntax(bp);
    view.wrap.syntax_to_wrap(sp).row
}

/// Visual-line up/down via the wrap conversions, preserving `c.desired_col`
/// across the move so moving through a ragged-right wrapped paragraph keeps
/// the caret in its visual column instead of snapping to each row's length.
fn move_row(
    view: &ViewSnapshots,
    buf: &rune_core::buffer::Buffer,
    c: Cursor,
    delta: isize,
    extend: Extend,
) -> Cursor {
    let origin_row = wrap_row_at(view, buf, c.position.get());
    let target_row = origin_row as isize + delta;

    let total = view.wrap.total_rows();
    let wp2 = if target_row < 0 {
        WrapPoint { row: 0, col: 0 }
    } else if total > 0 && target_row as usize >= total {
        let row = total - 1;
        WrapPoint {
            row,
            col: view.wrap.segment_len_at(row),
        }
    } else {
        let row = target_row as usize;
        let col = view
            .wrap
            .byte_col_from_visual(buf.content(), row, c.desired_col.0);
        WrapPoint { row, col }
    };

    let sp2 = view.wrap.wrap_to_syntax(buf.content(), wp2);
    let bp2 = view.syntax.syntax_to_buffer(sp2);
    let offset2 = BufferOffset(buf.line_col_to_offset(bp2));

    Cursor {
        position: offset2,
        anchor: if extend == Extend::Yes {
            c.anchor
        } else {
            offset2
        },
        desired_col: c.desired_col,
        id: c.id,
    }
}

/// A full viewport minus one row of overlap for context.
pub(crate) fn page_step(doc: &Document) -> isize {
    let h = doc.viewport.height;
    if h > 1 { (h - 1) as isize } else { 1 }
}

/// Shared vertical-motion driver (line up/down, page up/down). Runs two
/// passes over the same original cursor set, both through `move_row`.
/// Reveal is caret-driven, so a line whose reveal state depends on the
/// cursor (a heading's `# `, an inline code span's backticks, a fenced
/// block's fence) can reflow the instant the cursor arrives there; the
/// first pass exists only to produce that settled, post-move view. The
/// second pass then re-runs `move_row` from the ORIGINAL cursors — not the
/// first pass's result — against the settled view, so both the origin row
/// and the delta are measured in the same layout the user actually lands
/// in, never mixed across two layouts with a different number of revealed
/// lines above the caret.
fn move_row_cursors(doc: &mut Document, extend: Extend, delta: isize) {
    let view = doc.view();
    let original: Vec<Cursor> = doc.cursors.all().to_vec();

    let first_pass: Vec<Cursor> = original
        .iter()
        .map(|&c| move_row(&view, &doc.buffer, c, delta, extend))
        .collect();
    doc.cursors = CursorSet::new_from(&first_pass);

    let settled = doc.view();
    let final_pass: Vec<Cursor> = original
        .iter()
        .map(|&c| move_row(&settled, &doc.buffer, c, delta, extend))
        .collect();
    doc.cursors = CursorSet::new_from(&final_pass);
}

pub fn line_up(doc: &mut Document, extend: Extend) {
    move_row_cursors(doc, extend, -1);
}

pub fn line_down(doc: &mut Document, extend: Extend) {
    move_row_cursors(doc, extend, 1);
}

pub fn page_up(doc: &mut Document, extend: Extend) {
    let step = page_step(doc);
    move_row_cursors(doc, extend, -step);
}

pub fn page_down(doc: &mut Document, extend: Extend) {
    let step = page_step(doc);
    move_row_cursors(doc, extend, step);
}

fn cursor_wrap_row(doc: &Document, view: &ViewSnapshots) -> WrapRow {
    let primary = doc.cursors.primary();
    let bp = doc.buffer.offset_to_line_col(primary.position.get());
    let sp = view.syntax.buffer_to_syntax(bp);
    WrapRow(view.wrap.syntax_to_wrap(sp).row)
}

/// Moves `scroll_row` by `delta` DISPLAY rows (`scroll_row` indexes
/// `DisplaySnapshot::rows`, table borders included — not the wrap rows
/// directly), clamped to `[0, total_rows - 1]`, and arms
/// `ScrollMode::Independent` so the per-batch settle snaps the cursor onto
/// screen instead of scrolling the viewport back to it.
pub fn scroll_lines(doc: &mut Document, delta: isize) {
    let total = doc.view().display.total_rows();
    let max_row = DisplayRow(total.saturating_sub(1));
    let current = doc.viewport.scroll_row;
    doc.viewport.scroll_row = if delta >= 0 {
        current + delta as usize
    } else {
        current - (-delta) as usize
    }
    .min(max_row);
    doc.viewport.mode = ScrollMode::Independent;
}

pub fn scroll_line_up(doc: &mut Document) {
    scroll_lines(doc, -1);
}

pub fn scroll_line_down(doc: &mut Document) {
    scroll_lines(doc, 1);
}

fn half_page_step(doc: &Document) -> isize {
    (doc.viewport.height as isize / 2).max(1)
}

pub fn scroll_half_page_up(doc: &mut Document) {
    let step = half_page_step(doc);
    scroll_lines(doc, -step);
}

pub fn scroll_half_page_down(doc: &mut Document) {
    let step = half_page_step(doc);
    scroll_lines(doc, step);
}

/// Sets `scroll_row` directly to `target_row` — an absolute DISPLAY row,
/// not a delta — and arms `Independent` mode.
fn scroll_to(doc: &mut Document, target_row: DisplayRow) {
    let total = doc.view().display.total_rows();
    let max_row = DisplayRow(total.saturating_sub(1));
    doc.viewport.scroll_row = target_row.min(max_row);
    doc.viewport.mode = ScrollMode::Independent;
}

/// Scrolls the viewport so the DISPLAY row containing byte offset `target`
/// is visible — merge mode's own "jump to a hunk" primitive. Unlike every
/// other command in this module, never touches a cursor: moving it here
/// would pollute the next journal step's `cursors_before`, corrupting
/// undo's reopen-the-hunk-you-just-resolved behavior.
pub(crate) fn scroll_to_byte_offset(doc: &mut Document, target: usize) {
    let view = doc.view();
    let clamped = target.min(doc.buffer.content().len());
    let bp = doc.buffer.offset_to_line_col(clamped);
    let sp = view.syntax.buffer_to_syntax(bp);
    let wrap_row = WrapRow(view.wrap.syntax_to_wrap(sp).row);
    let row = view.display.wrap_to_display(wrap_row);
    scroll_to(doc, row);
}

pub fn centre_cursor(doc: &mut Document) {
    let view = doc.view();
    let row = view.display.wrap_to_display(cursor_wrap_row(doc, &view));
    let half = doc.viewport.height as usize / 2;
    scroll_to(doc, row - half);
}

pub fn cursor_to_top(doc: &mut Document) {
    let view = doc.view();
    let row = view.display.wrap_to_display(cursor_wrap_row(doc, &view));
    scroll_to(doc, row);
}

pub fn cursor_to_bottom(doc: &mut Document) {
    let view = doc.view();
    let row = view.display.wrap_to_display(cursor_wrap_row(doc, &view));
    let height = doc.viewport.height as usize;
    scroll_to(doc, row - height.saturating_sub(1));
}

pub fn scroll_to_document_top(doc: &mut Document) {
    scroll_to(doc, DisplayRow(0));
}

pub fn scroll_to_document_bottom(doc: &mut Document) {
    let total = doc.view().display.total_rows();
    let height = doc.viewport.height as usize;
    scroll_to(doc, DisplayRow(total.saturating_sub(height)));
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use rune_core::buffer::Buffer;

    fn doc_with_lines(n: usize, height: u16) -> Document {
        let content: String = (0..n).map(|i| format!("line {i}\n")).collect();
        let mut doc = Document::new(Buffer::new(content));
        doc.viewport.set_size(80, height);
        doc.viewport.scrolloff = 0;
        doc.cursors = CursorSet::new(0);
        doc
    }

    #[test]
    fn scroll_line_down_moves_viewport_not_cursor() {
        let mut doc = doc_with_lines(100, 10);
        let cursor_before = doc.cursors.primary().position;
        scroll_line_down(&mut doc);
        assert_eq!(doc.viewport.scroll_row, DisplayRow(1));
        assert_eq!(doc.viewport.mode, ScrollMode::Independent);
        assert_eq!(doc.cursors.primary().position, cursor_before);
    }

    #[test]
    fn scroll_line_up_is_clamped_at_the_top() {
        let mut doc = doc_with_lines(100, 10);
        scroll_line_up(&mut doc);
        assert_eq!(doc.viewport.scroll_row, DisplayRow(0));
    }

    #[test]
    fn scroll_half_page_down_moves_half_the_viewport_height() {
        let mut doc = doc_with_lines(100, 20);
        scroll_half_page_down(&mut doc);
        assert_eq!(doc.viewport.scroll_row, DisplayRow(10));
    }

    #[test]
    fn centre_cursor_puts_the_cursor_row_in_the_middle() {
        let mut doc = doc_with_lines(100, 20);
        let offset = doc
            .buffer
            .line_start(50)
            .expect("line 50 exists in a 100-line fixture");
        doc.cursors = CursorSet::new(offset);
        centre_cursor(&mut doc);
        assert_eq!(doc.viewport.scroll_row, DisplayRow(40));
    }

    #[test]
    fn cursor_to_top_scrolls_the_cursor_row_to_row_zero() {
        let mut doc = doc_with_lines(100, 20);
        let offset = doc
            .buffer
            .line_start(50)
            .expect("line 50 exists in a 100-line fixture");
        doc.cursors = CursorSet::new(offset);
        cursor_to_top(&mut doc);
        assert_eq!(doc.viewport.scroll_row, DisplayRow(50));
    }

    #[test]
    fn cursor_to_bottom_scrolls_the_cursor_row_to_the_last_visible_row() {
        let mut doc = doc_with_lines(100, 20);
        let offset = doc
            .buffer
            .line_start(50)
            .expect("line 50 exists in a 100-line fixture");
        doc.cursors = CursorSet::new(offset);
        cursor_to_bottom(&mut doc);
        assert_eq!(doc.viewport.scroll_row, DisplayRow(50 - 19));
    }

    #[test]
    fn scroll_to_document_top_lands_on_row_zero() {
        let mut doc = doc_with_lines(100, 20);
        doc.viewport.scroll_row = DisplayRow(42);
        scroll_to_document_top(&mut doc);
        assert_eq!(doc.viewport.scroll_row, DisplayRow(0));
        assert_eq!(doc.viewport.mode, ScrollMode::Independent);
    }

    #[test]
    fn scroll_to_document_bottom_lands_on_the_last_full_page() {
        let mut doc = doc_with_lines(100, 20);
        scroll_to_document_bottom(&mut doc);
        let total = doc.view().display.total_rows();
        assert_eq!(doc.viewport.scroll_row, DisplayRow(total - 20));
    }
}
