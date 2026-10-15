use rune_core::coords::VisualCol;
use rune_md::snapshot::SnapshotRow;

use super::Cell;

pub(super) fn scroll_columns<'a>(
    rows: &mut [Vec<Cell>],
    display_rows: impl Iterator<Item = &'a SnapshotRow>,
    scroll_col: VisualCol,
) {
    if scroll_col.0 == 0 {
        return;
    }
    for (cells, display) in rows.iter_mut().zip(display_rows) {
        if display.image.is_none() {
            drop_leading_columns(cells, scroll_col.0);
        }
    }
}

fn drop_leading_columns(cells: &mut Vec<Cell>, columns: usize) {
    let mut acc = 0usize;
    let mut kept = Vec::with_capacity(cells.len());
    for cell in cells.drain(..) {
        let width = usize::from(cell.width);
        if acc >= columns {
            kept.push(cell);
        } else if acc + width > columns {
            for _ in 0..acc + width - columns {
                kept.push(Cell {
                    text: " ".into(),
                    width: 1,
                    style: cell.style,
                    buf_offset: cell.buf_offset,
                });
            }
        }
        acc += width;
    }
    *cells = kept;
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use ratatui::style::Style;

    use super::*;

    fn cell(text: &str, width: u8, buf_offset: u32) -> Cell {
        Cell {
            text: text.into(),
            width,
            style: Style::default(),
            buf_offset: Some(buf_offset),
        }
    }

    fn texts(cells: &[Cell]) -> String {
        cells.iter().map(|c| c.text.as_str()).collect()
    }

    #[test]
    fn leading_columns_are_dropped_by_cell_width() {
        let mut cells = vec![cell("a", 1, 0), cell("b", 1, 1), cell("c", 1, 2)];
        drop_leading_columns(&mut cells, 2);
        assert_eq!(texts(&cells), "c");
    }

    #[test]
    fn a_wide_cell_cut_in_half_leaves_one_blank_column_that_keeps_its_offset() {
        let mut cells = vec![cell("a", 1, 0), cell("\u{4f60}", 2, 1), cell("b", 1, 4)];
        drop_leading_columns(&mut cells, 2);
        assert_eq!(texts(&cells), " b");
        assert_eq!(cells[0].width, 1);
        assert_eq!(cells[0].buf_offset, Some(1));
    }

    #[test]
    fn scrolling_past_the_end_of_a_short_row_leaves_it_empty() {
        let mut cells = vec![cell("a", 1, 0)];
        drop_leading_columns(&mut cells, 9);
        assert!(cells.is_empty());
    }

    #[test]
    fn a_zero_width_cell_at_the_cut_travels_with_the_cell_after_it() {
        let mut cells = vec![cell("a", 1, 0), cell("\u{200b}", 0, 1), cell("b", 1, 4)];
        drop_leading_columns(&mut cells, 1);
        assert_eq!(texts(&cells), "\u{200b}b");
    }
}
