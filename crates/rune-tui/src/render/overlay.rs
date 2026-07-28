//! Cursor/selection overlays, split out of `render` (§1.6 budget):
//! `build_rows` calls `apply_cursor_overlays` after collecting a row's plain
//! `segment_cells`, patching in the selection background and the caret's
//! reverse-video, exactly as it did when these functions lived in `render`
//! itself.

use ratatui::style::{Modifier as RtModifier, Style};

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_md::element::doc::ViewSnapshots;

use crate::theme::Theme;

use super::Cell;

pub(super) fn apply_cursor_overlays(
    rows: &mut [Vec<Cell>],
    view: &ViewSnapshots,
    cursors: &CursorSet,
    buf: &Buffer,
    scroll_row: usize,
    theme: &Theme,
) {
    for cursor in cursors.all() {
        if cursor.has_selection() {
            let (start, end) = cursor.selection_range();
            highlight_selection(rows, start, end, theme);
        }

        let buffer_point = buf.offset_to_line_col(cursor.position);
        let syntax_point = view.syntax.buffer_to_syntax(buffer_point);
        let wrap_point = view.wrap.syntax_to_wrap(syntax_point);
        // The cursor's own row lives in WRAP space (border rows aren't
        // addressable by the caret); convert to the DISPLAY row `rows` is
        // now indexed by before comparing against/indexing off `scroll_row`
        // (also display-space, WP3.S5).
        let display_row = view.display.wrap_to_display(wrap_point.row);
        if display_row < scroll_row {
            continue;
        }
        let Some(row) = rows.get_mut(display_row - scroll_row) else {
            continue;
        };
        let visual_col = view
            .wrap
            .visual_col(buf.content(), wrap_point.row, wrap_point.col);
        place_caret(row, visual_col, cursor.position);
    }
}

fn highlight_selection(rows: &mut [Vec<Cell>], start: usize, end: usize, theme: &Theme) {
    for row in rows.iter_mut() {
        for cell in row.iter_mut() {
            if cell.buf_offset >= 0 {
                let offset = cell.buf_offset as usize;
                if offset >= start && offset < end {
                    // Go `Selection` (`styles.go:196`, WP2.S2 migration).
                    cell.style = cell.style.bg(theme.chrome.selection_bg);
                }
            }
        }
    }
}

/// Reverse-video the cell at `visual_col`, or — if the caret sits past the
/// last visible char on this row — append a synthetic EOL cursor cell (port
/// of Go `render.go:151-176`).
fn place_caret(row: &mut Vec<Cell>, visual_col: usize, buf_offset: usize) {
    let mut col = 0usize;
    for cell in row.iter_mut() {
        if col == visual_col {
            cell.style = cell.style.add_modifier(RtModifier::REVERSED);
            return;
        }
        col += cell.width.max(1) as usize;
    }
    row.push(Cell {
        text: " ".to_string(),
        width: 1,
        style: Style::default().add_modifier(RtModifier::REVERSED),
        buf_offset: buf_offset as i64,
    });
}
