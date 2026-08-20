use unicode_segmentation::UnicodeSegmentation;

use rune_md::snapshot::SnapshotRow;
use rune_syntax::wrap::grapheme_width;

use crate::theme::Theme;

use super::Cell;

pub fn decor_row_cells(theme: &Theme, row: &SnapshotRow) -> Vec<Cell> {
    let Some(decor) = row.decor.as_ref() else {
        return Vec::new();
    };
    let mut cells = Vec::new();
    for piece in &decor.pieces {
        let style = theme.scope_style(piece.scope);
        for grapheme in piece.text.graphemes(true) {
            let width = grapheme_width(grapheme);
            cells.push(Cell {
                text: grapheme.into(),
                width: width as u8,
                style,
                buf_offset: None,
            });
        }
    }
    cells
}

pub fn decor_cell_width(row: &SnapshotRow) -> u16 {
    row.decor.as_ref().map_or(0, |d| d.cells as u16)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use rune_syntax::scope::scope_table;
    use rune_syntax::wrap::{SegDecor, SegDecorPiece};

    fn row_with_decor(decor: Option<SegDecor>) -> SnapshotRow {
        SnapshotRow {
            spans: Vec::new(),
            wrap_row: 0,
            synthetic: false,
            decor,
            image: None,
        }
    }

    #[test]
    fn no_decor_produces_no_cells_and_zero_width() {
        let theme = Theme::catppuccin_mocha(false);
        let row = row_with_decor(None);
        assert!(decor_row_cells(&theme, &row).is_empty());
        assert_eq!(decor_cell_width(&row), 0);
    }

    #[test]
    fn decor_cells_carry_no_buf_offset_and_the_pieces_style() {
        let theme = Theme::catppuccin_mocha(false);
        let scope = scope_table().resolve("markup.heading.1").unwrap();
        let decor = SegDecor {
            pieces: vec![SegDecorPiece {
                text: "\u{25C9} ".to_string(),
                scope,
            }],
            cells: 2,
        };
        let row = row_with_decor(Some(decor));
        let cells = decor_row_cells(&theme, &row);
        assert_eq!(decor_cell_width(&row), 2);
        let total_width: usize = cells.iter().map(|c| c.width as usize).sum();
        assert_eq!(total_width, 2);
        for cell in &cells {
            assert_eq!(cell.buf_offset, None);
            assert_eq!(cell.style, theme.scope_style(scope));
        }
    }
}
