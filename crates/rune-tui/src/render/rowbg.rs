//! The left column's row-background chokepoint (Explorer/Tabs), a sibling
//! of `code_bg`'s region-rectangle fill but for a single ROW rather than a
//! multi-row code block: [`fill_row`] paints a rect after `render_widget`
//! has already placed the row's text, rather than tinting a span's own
//! `bg`. A span can only colour cells that exist, so it stops at the last
//! character of a short file/tab name and leaves the ragged space past it
//! uncovered — the cursor/active-document bar would read as a highlight
//! that stops mid-air instead of a full row.
//!
//! This is deliberately NOT `render::paint_range`: `paint_range` walks the
//! editor's own `Cell` row model (`render::cell`'s buffer-content walk),
//! built fresh every frame from a document's wrap segments. The Explorer
//! and Tabs panes have no such model — they render straight to the
//! ratatui `Buffer` through `Paragraph`/`render_widget` — so there is no
//! `Cell` row for `paint_range` to walk here. `code_bg::fill_row` is the
//! nearer relative (also a post-widget rect fill) but operates on that
//! same `Cell` row list, not a ratatui `Rect`; this module's `fill_row`
//! is the one that reaches straight into `frame.buffer_mut()`.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;

/// Fills `row` with `style` on the live frame buffer, called AFTER the
/// row's own `render_widget` call so the background lands on top of
/// whatever glyphs are already there. `Buffer::set_style` intersects `row`
/// with the buffer's own area before writing, so a row rect that runs past
/// the terminal edge is clipped rather than panicking; `style`'s `bg` is
/// the only field callers here ever set, and `Cell::set_style` only
/// assigns a field when the patching `Style` actually carries one, so
/// every cell keeps its own foreground and modifiers untouched.
pub fn fill_row(frame: &mut Frame, row: Rect, style: Style) {
    frame.buffer_mut().set_style(row, style);
}
