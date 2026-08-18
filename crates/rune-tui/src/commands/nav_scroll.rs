//! Vertical/page motion (split out of `nav.rs` to stay under the 500-line
//! budget) plus the
//! viewport-only scroll commands: `scroll_line_up`/`down`,
//! `scroll_half_page_up`/`down`, `centre_cursor`, `cursor_to_top`,
//! `cursor_to_bottom`.
//!
//! `line_up`/`line_down`/`page_up`/`page_down` are CURSOR-driven: the
//! cursor moves and `Document::sync`'s `scroll_to_cursor` -> `Viewport::
//! reconcile` chases it afterward (`ScrollMode::FollowCursor`, the
//! default). The new scroll commands below are the opposite: they move
//! `Viewport::scroll_row` directly and arm `ScrollMode::Independent`, so
//! the SAME `reconcile` call instead snaps the cursor onto screen if the
//! scroll pushed it out of the scrolloff-padded band — never scrolling the
//! viewport back to the cursor, which would defeat the point of scrolling
//! (vim `runtime/doc/scroll.txt`; Helix `commands::scroll(..., sync_cursor:
//! false)`). Neither family moves the viewport or the cursor a second time
//! themselves — `reconcile` is the sole writer of `scroll_row`, and
//! `Document::snap_cursor_to_row` is the sole `Independent`-mode writer of
//! the cursor (see `document.rs`'s docs).

use rune_core::coords::{DisplayRow, WrapPoint, WrapRow};
use rune_core::cursor::{Cursor, CursorSet};
use rune_md::element::doc::ViewSnapshots;

use crate::document::Document;
use crate::keymap::Extend;
use crate::viewport::ScrollMode;

/// Visual-line up/down via the wrap conversions, preserving `c.desired_col`
/// across the move (the property that makes moving through a ragged-right
/// wrapped paragraph keep the caret in its visual column instead of
/// snapping to each row's length).
fn move_row(
    view: &ViewSnapshots,
    buf: &rune_core::buffer::Buffer,
    c: Cursor,
    delta: isize,
    extend: Extend,
) -> Cursor {
    let bp = buf.offset_to_line_col(c.position);
    let sp = view.syntax.buffer_to_syntax(bp);
    let wp = view.wrap.syntax_to_wrap(sp);
    let target_row = wp.row as isize + delta;

    let total = view.wrap.total_rows();
    let wp2 = if target_row < 0 {
        WrapPoint { row: 0, col: 0 }
    } else if total > 0 && target_row as usize >= total {
        // Clamped past the last row: land at that row's own end —
        // `segment_len_at` expresses the "end of row" intent directly,
        // without a magic-number sentinel.
        let row = total - 1;
        WrapPoint {
            row,
            col: view.wrap.segment_len_at(row),
        }
    } else {
        let row = target_row as usize;
        let col = view
            .wrap
            .byte_col_from_visual(buf.content(), row, c.desired_col);
        WrapPoint { row, col }
    };

    let sp2 = view.wrap.wrap_to_syntax(buf.content(), wp2);
    let bp2 = view.syntax.syntax_to_buffer(sp2);
    let offset2 = buf.line_col_to_offset(bp2);

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

/// A full viewport minus one row of
/// overlap for context. `pub(crate)` so `commands::reading_nav` pages a
/// read-only document by the exact same step `page_up`/`page_down` use
/// below, rather than re-deriving it.
pub(crate) fn page_step(doc: &Document) -> isize {
    let h = doc.viewport.height;
    if h > 1 { (h - 1) as isize } else { 1 }
}

/// Shared vertical-motion driver (line up/down, page up/down). Two passes:
/// the first moves each cursor using the PRE-move view (reveal keyed off
/// the old cursor position), which finds the right row but can misjudge the
/// column on a line whose reveal state is itself cursor-driven (a heading's
/// `# ` conceals until the caret lands on it) — landing there changes that
/// line's own wrap layout the instant the cursor arrives. The second pass
/// re-views (now reflecting the moved cursor) and resnaps each cursor's
/// column from `desired_col` against that settled layout, so the caret ends
/// up where the user now sees it, not where the stale layout put it.
fn move_row_cursors(doc: &mut Document, extend: Extend, delta: isize) {
    let view = doc.view();
    let new_cursors: Vec<Cursor> = doc
        .cursors
        .all()
        .iter()
        .map(|&c| move_row(&view, &doc.buffer, c, delta, extend))
        .collect();
    doc.cursors = CursorSet::new_from(&new_cursors);

    let settled = doc.view();
    let resettled: Vec<Cursor> = doc
        .cursors
        .all()
        .iter()
        .map(|&c| resettle_col(&settled, &doc.buffer, c, extend))
        .collect();
    doc.cursors = CursorSet::new_from(&resettled);
}

/// Re-derives a cursor's byte position from its own `desired_col` against
/// `view`, keeping it on the same wrap row — the settle half of
/// `move_row_cursors`'s two-pass fixup.
fn resettle_col(
    view: &ViewSnapshots,
    buf: &rune_core::buffer::Buffer,
    c: Cursor,
    extend: Extend,
) -> Cursor {
    let bp = buf.offset_to_line_col(c.position);
    let sp = view.syntax.buffer_to_syntax(bp);
    let wp = view.wrap.syntax_to_wrap(sp);
    let col = view
        .wrap
        .byte_col_from_visual(buf.content(), wp.row, c.desired_col);
    let sp2 = view
        .wrap
        .wrap_to_syntax(buf.content(), WrapPoint { row: wp.row, col });
    let bp2 = view.syntax.syntax_to_buffer(sp2);
    let offset2 = buf.line_col_to_offset(bp2);
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

/// The row the PRIMARY cursor currently sits on, in wrap space — the input
/// every viewport-only scroll command below needs before it can compute
/// where to put `scroll_row`.
fn cursor_wrap_row(doc: &Document, view: &ViewSnapshots) -> WrapRow {
    let primary = doc.cursors.primary();
    let bp = doc.buffer.offset_to_line_col(primary.position);
    let sp = view.syntax.buffer_to_syntax(bp);
    WrapRow(view.wrap.syntax_to_wrap(sp).row)
}

/// Moves `scroll_row` by `delta` DISPLAY rows (`scroll_row` indexes
/// `DisplaySnapshot::rows`, table borders included — not the wrap rows
/// directly), clamped to `[0, total_rows - 1]` (never scrolled past the
/// document), and arms `ScrollMode::Independent` so the
/// per-batch settle snaps the cursor onto screen instead of scrolling the
/// viewport back to it. The shared chokepoint both the line-scroll commands
/// below and the mouse wheel (`commands::mouse`: the wheel scrolls 3
/// rows per notch) route through, so the two can never disagree about how a scroll
/// clamps or arms `Independent` mode.
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

/// vim `ctrl+e`/Helix `scroll_line_up` — viewport-only, one row.
pub fn scroll_line_up(doc: &mut Document) {
    scroll_lines(doc, -1);
}

/// vim `ctrl+y`/Helix `scroll_line_down` — viewport-only, one row.
pub fn scroll_line_down(doc: &mut Document) {
    scroll_lines(doc, 1);
}

fn half_page_step(doc: &Document) -> isize {
    (doc.viewport.height as isize / 2).max(1)
}

/// Helix `half_page_up`: `commands::scroll(..., sync_cursor: false)` — the
/// viewport moves by half a page; the cursor only follows if the scroll
/// pushed it out of view.
pub fn scroll_half_page_up(doc: &mut Document) {
    let step = half_page_step(doc);
    scroll_lines(doc, -step);
}

/// Helix `half_page_down` — the mirror of `scroll_half_page_up`.
pub fn scroll_half_page_down(doc: &mut Document) {
    let step = half_page_step(doc);
    scroll_lines(doc, step);
}

/// Sets `scroll_row` directly to `target_row` (a DISPLAY row — not a delta)
/// and arms `Independent` mode — shared by `centre_cursor`/`cursor_to_top`/
/// `cursor_to_bottom` below, each of which converts the cursor's own WRAP
/// row through `DisplaySnapshot::wrap_to_display` before calling this.
fn scroll_to(doc: &mut Document, target_row: DisplayRow) {
    let total = doc.view().display.total_rows();
    let max_row = DisplayRow(total.saturating_sub(1));
    doc.viewport.scroll_row = target_row.min(max_row);
    doc.viewport.mode = ScrollMode::Independent;
}

/// Scrolls the viewport so the DISPLAY row containing byte offset `target`
/// is visible — merge mode's own "jump to a hunk" primitive. Converts
/// through the exact same buffer -> syntax ->
/// wrap -> display chokepoint chain `cursor_wrap_row`/`scroll_to` above use
/// for the cursor's own position, but starting from a raw byte offset
/// instead — and, unlike every other command in this module, never touches
/// a cursor: `[`/`]` navigation moving the cursor would pollute the NEXT
/// journal `Step`'s `cursors_before`, corrupting undo's "reopen the hunk
/// you just resolved" experience.
pub(crate) fn scroll_to_byte_offset(doc: &mut Document, target: usize) {
    let view = doc.view();
    let clamped = target.min(doc.buffer.content().len());
    let bp = doc.buffer.offset_to_line_col(clamped);
    let sp = view.syntax.buffer_to_syntax(bp);
    let wrap_row = WrapRow(view.wrap.syntax_to_wrap(sp).row);
    let row = view.display.wrap_to_display(wrap_row);
    scroll_to(doc, row);
}

/// vim/Helix `zz`: re-centres the viewport on the cursor's current row
/// without moving the cursor itself.
pub fn centre_cursor(doc: &mut Document) {
    let view = doc.view();
    let row = view.display.wrap_to_display(cursor_wrap_row(doc, &view));
    let half = doc.viewport.height as usize / 2;
    scroll_to(doc, row - half);
}

/// vim/Helix `zt`: scrolls the cursor's row to the top of the viewport.
pub fn cursor_to_top(doc: &mut Document) {
    let view = doc.view();
    let row = view.display.wrap_to_display(cursor_wrap_row(doc, &view));
    scroll_to(doc, row);
}

/// vim/Helix `zb`: scrolls the cursor's row to the bottom of the viewport.
pub fn cursor_to_bottom(doc: &mut Document) {
    let view = doc.view();
    let row = view.display.wrap_to_display(cursor_wrap_row(doc, &view));
    let height = doc.viewport.height as usize;
    scroll_to(doc, row - height.saturating_sub(1));
}

/// `commands::reading_nav`'s `Home`: scrolls a read-only document to its
/// very first row.
pub fn scroll_to_document_top(doc: &mut Document) {
    scroll_to(doc, DisplayRow(0));
}

/// `commands::reading_nav`'s `End`: scrolls a read-only document to its
/// LAST PAGE, not merely its last row — the document-reader convention
/// (Preview, Evince, iBooks all land End on a full final page, not one row
/// of content above a screen of blank space).
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
        // Move the cursor to line 50 directly.
        let offset = doc
            .buffer
            .line_start(50)
            .expect("line 50 exists in a 100-line fixture");
        doc.cursors = CursorSet::new(offset);
        centre_cursor(&mut doc);
        assert_eq!(doc.viewport.scroll_row, DisplayRow(40)); // 50 - 20/2
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
