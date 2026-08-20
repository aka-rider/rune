use ratatui::Frame;
use ratatui::buffer::CellWidth;
use ratatui::layout::Rect;
use rune_core::assert_invariant;

use super::Cell;

// ratatui's own diffing (BufferDiff) skips re-examining the continuation
// column(s) after any cell whose own cell_width() is > 1, assuming a
// double-width cell is never followed by non-blank content — so every
// continuation column of a wide Cell must be explicitly reset here, or
// stale content there would never reach the real terminal's redraw.
pub fn blit(rows: &[Vec<Cell>], area: Rect, frame: &mut Frame) {
    let buf = frame.buffer_mut();
    let right = area.x.saturating_add(area.width);
    for (row_idx, row) in rows.iter().enumerate() {
        let y = area.y.saturating_add(row_idx as u16);
        if y >= area.y.saturating_add(area.height) {
            break;
        }
        let mut x = area.x;
        for cell in row {
            if x >= right {
                break;
            }
            assert_invariant!(
                {
                    let declared = usize::from(cell.width);
                    let ratatui_width = cell.text.cell_width() as usize;
                    declared == ratatui_width
                        || (declared == 1
                            && ratatui_width == 0
                            && cell.text.chars().count() > 1)
                },
                || {
                    format!(
                        "Cell width {} disagrees with ratatui's own cell_width() {} for symbol {:?}",
                        cell.width,
                        cell.text.cell_width(),
                        cell.text
                    )
                },
            );
            let width = u16::from(cell.width);
            let fits = x.saturating_add(width) <= right;
            if let Some(target) = buf.cell_mut((x, y)) {
                if fits {
                    target.set_symbol(&cell.text);
                } else {
                    target.set_symbol(" ");
                }
                target.set_style(cell.style);
            }
            if fits {
                for dx in 1..width {
                    let cx = x.saturating_add(dx);
                    if cx >= right {
                        break;
                    }
                    if let Some(cont) = buf.cell_mut((cx, y)) {
                        cont.reset();
                    }
                }
            }
            x = x.saturating_add(width);
        }
    }
}
