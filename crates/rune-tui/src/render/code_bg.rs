//! Paints a code region's background as a RECTANGLE (§1.6 split of the
//! render module): every display row belonging to a `CodeRegion` is filled
//! with `Theme::chrome.code_bg` from the end of its own decoration prefix to
//! the right edge of the pane.
//!
//! A background carried on a `SyntaxSpan` cannot do this. A span's `bg` can
//! only colour cells that EXIST, and cells are emitted only for real span
//! text, so the tint stopped at each line's last character, vanished
//! entirely on a blank line inside a block (an empty content range emits no
//! span and therefore no cell), and never appeared behind a whole code
//! document, whose text carries no fence scope at all. Region membership is
//! a property of the LINE, not of the bytes on it, so the fill is driven off
//! `CodeRegion::rows` — the one definition of code — and the missing columns
//! are appended as real padding cells, since the blit writes nothing past a
//! row's last cell.
//!
//! Two properties are load-bearing and deliberately structural rather than
//! checked after the fact:
//!
//! - The fill is a pure function of the display snapshot, the regions, and
//!   the pane width. It never consults highlight state, so a `Msg::
//!   Highlighted` step cannot change any row's cell count (`HL-NO-REFLOW`)
//!   and two message-free renders of the same state agree cell for cell
//!   (`SYNC-IDEMPOTENT`).
//! - It starts at the row's own decoration width, so a fence inside a
//!   blockquote paints AFTER the quote bar and never under it; and it skips
//!   every table, synthetic and image row outright, so it can never grow a
//!   boxed table row by a cell (`TABLE-ROW-WIDTH`) nor disturb a placeholder
//!   row whose style smuggles an image id.
//!
//! Padding cells carry the `-1` "no buffer correspondence" sentinel every
//! other decorative cell uses: they claim no byte for the caret, selection
//! or click hit-testing to resolve to, and they stay out of the min/max
//! `buf_offset` hull the per-frame highlight query window is derived from,
//! which a real offset would silently widen.

use ratatui::style::{Color, Style};

use rune_md::element::code_region::CodeRegion;
use rune_md::element::doc::ViewSnapshots;

use super::Cell;
use super::decor::decor_cell_width;

/// Fills `[decor_cell_width(row), width)` with `bg` on every row of `rows`
/// whose source (model) line falls inside some region's `rows` span.
///
/// `rows` is positional: `rows[i]` is display row `scroll_row + i`, the same
/// window `build_rows` itself sliced. A row's source line comes from the
/// wrap segment it was built from; `WrapSegment::model_line` is constant
/// across a wrapped line's continuation segments, so a wrapped code line's
/// continuation rows are covered without a special case.
pub(super) fn paint_code_background(
    rows: &mut [Vec<Cell>],
    view: &ViewSnapshots,
    scroll_row: usize,
    width: u16,
    regions: &[CodeRegion],
    bg: Color,
) {
    if regions.is_empty() || width == 0 {
        return;
    }
    let display_rows = view.display.rows();
    let segments = view.wrap.segments();
    for (i, cells) in rows.iter_mut().enumerate() {
        let Some(row) = display_rows.get(scroll_row.saturating_add(i)) else {
            continue;
        };
        // A synthesised table border has no source line to belong to a
        // region, an image row's cells are placeholders whose geometry the
        // image protocol depends on, and a table content row shares one
        // summed width with every other row in its box. Code regions
        // contain none of these — skipping them is what makes that true by
        // construction instead of by assumption.
        if row.synthetic || row.image.is_some() {
            continue;
        }
        let Some(segment) = segments.get(row.wrap_row) else {
            continue;
        };
        if segment.table.is_some() {
            continue;
        }
        if !regions
            .iter()
            .any(|region| region.rows.contains(&segment.model_line))
        {
            continue;
        }
        fill_row(cells, decor_cell_width(row) as usize, width as usize, bg);
    }
}

/// Backgrounds `cells` from column `start_col` to `width`, appending
/// single-cell padding for every column past the last emitted cell.
///
/// Columns before `start_col` are the row's decoration prefix and keep their
/// own style untouched — that is what puts a blockquote's bar in front of
/// the background rather than under it. The column walk sums `Cell::width`
/// (terminal CELLS, §1.5), never a byte or `char` count, so a wide glyph
/// advances the cursor by the columns it actually occupies.
fn fill_row(cells: &mut Vec<Cell>, start_col: usize, width: usize, bg: Color) {
    let mut col = 0usize;
    for cell in cells.iter_mut() {
        if col >= start_col && col < width {
            cell.style = cell.style.bg(bg);
        }
        col = col.saturating_add(usize::from(cell.width.max(1)));
    }
    while col < width {
        cells.push(Cell {
            text: " ".to_string(),
            width: 1,
            style: Style::default().bg(bg),
            buf_offset: -1,
        });
        col = col.saturating_add(1);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn cell(text: &str, width: u8, buf_offset: i64) -> Cell {
        Cell {
            text: text.to_string(),
            width,
            style: Style::default(),
            buf_offset,
        }
    }

    #[test]
    fn an_empty_row_is_filled_to_the_pane_width_with_decorative_padding() {
        let bg = Color::Rgb(1, 2, 3);
        let mut cells: Vec<Cell> = Vec::new();
        fill_row(&mut cells, 0, 6, bg);
        assert_eq!(cells.len(), 6);
        for c in &cells {
            assert_eq!(c.buf_offset, -1, "padding must claim no buffer byte");
            assert_eq!(c.width, 1);
            assert_eq!(c.style.bg, Some(bg));
        }
    }

    #[test]
    fn a_short_row_keeps_its_cells_and_grows_to_the_pane_width() {
        let bg = Color::Rgb(1, 2, 3);
        let mut cells = vec![cell("a", 1, 0), cell("b", 1, 1)];
        fill_row(&mut cells, 0, 5, bg);
        assert_eq!(cells.len(), 5);
        assert_eq!(cells[0].text, "a");
        assert_eq!(cells[0].buf_offset, 0);
        for c in &cells {
            assert_eq!(c.style.bg, Some(bg));
        }
    }

    #[test]
    fn columns_before_the_decor_prefix_keep_their_own_style() {
        let bg = Color::Rgb(1, 2, 3);
        let mut cells = vec![cell("\u{2502}", 1, -1), cell(" ", 1, -1), cell("x", 1, 0)];
        fill_row(&mut cells, 2, 5, bg);
        assert_eq!(cells[0].style.bg, None, "the quote bar must stay uncovered");
        assert_eq!(cells[1].style.bg, None);
        assert_eq!(cells[2].style.bg, Some(bg));
        assert_eq!(cells.len(), 5);
    }

    /// A wide glyph advances the column cursor by the CELLS it occupies, so
    /// the row still ends exactly at the pane width rather than one column
    /// past it.
    #[test]
    fn a_wide_glyph_advances_by_its_cell_width_not_by_one() {
        let bg = Color::Rgb(1, 2, 3);
        let mut cells = vec![cell("\u{4E00}", 2, 0)];
        fill_row(&mut cells, 0, 4, bg);
        let total: usize = cells.iter().map(|c| usize::from(c.width)).sum();
        assert_eq!(total, 4);
    }

    /// A row already at or past the pane width gains nothing — the fill
    /// never widens a row beyond the pane.
    #[test]
    fn a_full_row_gains_no_padding() {
        let bg = Color::Rgb(1, 2, 3);
        let mut cells: Vec<Cell> = (0..4).map(|i| cell("x", 1, i)).collect();
        fill_row(&mut cells, 0, 4, bg);
        assert_eq!(cells.len(), 4);
    }
}
