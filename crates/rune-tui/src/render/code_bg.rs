use ratatui::style::{Color, Style};

use rune_core::coords::DisplayRow;
use rune_md::element::code_region::CodeRegion;
use rune_md::element::doc::ViewSnapshots;

use super::Cell;
use super::decor::decor_cell_width;

pub(super) fn paint_code_background(
    rows: &mut [Vec<Cell>],
    view: &ViewSnapshots,
    scroll_row: DisplayRow,
    width: u16,
    bg: Color,
) {
    let regions: &[CodeRegion] = &view.code_regions;
    if regions.is_empty() || width == 0 {
        return;
    }
    let display_rows = view.display.rows();
    let segments = view.wrap.segments();
    for (i, cells) in rows.iter_mut().enumerate() {
        let Some(row) = display_rows.get((scroll_row + i).0) else {
            continue;
        };
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
            text: " ".into(),
            width: 1,
            style: Style::default().bg(bg),
            buf_offset: None,
        });
        col = col.saturating_add(1);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn cell(text: &str, width: u8, buf_offset: Option<u32>) -> Cell {
        Cell {
            text: text.into(),
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
            assert_eq!(c.buf_offset, None, "padding must claim no buffer byte");
            assert_eq!(c.width, 1);
            assert_eq!(c.style.bg, Some(bg));
        }
    }

    #[test]
    fn a_short_row_keeps_its_cells_and_grows_to_the_pane_width() {
        let bg = Color::Rgb(1, 2, 3);
        let mut cells = vec![cell("a", 1, Some(0)), cell("b", 1, Some(1))];
        fill_row(&mut cells, 0, 5, bg);
        assert_eq!(cells.len(), 5);
        assert_eq!(cells[0].text, "a");
        assert_eq!(cells[0].buf_offset, Some(0));
        for c in &cells {
            assert_eq!(c.style.bg, Some(bg));
        }
    }

    #[test]
    fn columns_before_the_decor_prefix_keep_their_own_style() {
        let bg = Color::Rgb(1, 2, 3);
        let mut cells = vec![
            cell("\u{2502}", 1, None),
            cell(" ", 1, None),
            cell("x", 1, Some(0)),
        ];
        fill_row(&mut cells, 2, 5, bg);
        assert_eq!(cells[0].style.bg, None, "the quote bar must stay uncovered");
        assert_eq!(cells[1].style.bg, None);
        assert_eq!(cells[2].style.bg, Some(bg));
        assert_eq!(cells.len(), 5);
    }

    #[test]
    fn a_wide_glyph_advances_by_its_cell_width_not_by_one() {
        let bg = Color::Rgb(1, 2, 3);
        let mut cells = vec![cell("\u{4E00}", 2, Some(0))];
        fill_row(&mut cells, 0, 4, bg);
        let total: usize = cells.iter().map(|c| usize::from(c.width)).sum();
        assert_eq!(total, 4);
    }

    #[test]
    fn a_full_row_gains_no_padding() {
        let bg = Color::Rgb(1, 2, 3);
        let mut cells: Vec<Cell> = (0..4).map(|i| cell("x", 1, Some(i))).collect();
        fill_row(&mut cells, 0, 4, bg);
        assert_eq!(cells.len(), 4);
    }
}
