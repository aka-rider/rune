//! Renders a `SnapshotRow`'s own line decoration (heading icon / list bullet
//! / quote bar / hr rule, `rune_syntax::wrap::decor::SegDecor`) into the
//! prefix `Cell`s `build_rows` prepends to that row before any overlay
//! walk runs (500-line budget split of the render module). A decoration cell carries no
//! buffer position (`buf_offset: -1`, the same sentinel the table layout's
//! synthetic border cells already use) — decoration is metadata carried
//! alongside a wrap segment, never a substitute for the segment's own
//! spans, so it never claims a byte the caret, selection, or click
//! hit-testing could resolve to.
//!
//! Decoration also marks where a row's CONTENT begins: the code-region
//! background rectangle fills from `decor_cell_width` rightwards, which is
//! what puts a blockquoted fence's background after the quote bar rather
//! than under it.

use unicode_segmentation::UnicodeSegmentation;

use rune_md::snapshot::SnapshotRow;
use rune_syntax::wrap::grapheme_width;

use crate::theme::Theme;

use super::Cell;

/// Builds `row`'s decoration prefix as `Cell`s, one grapheme cluster at a
/// time through the same width chokepoint every other cell walk in this
/// module uses (`grapheme_width`, `push_grapheme_cells`'s docs) — an empty
/// `Vec` when the row carries no decor. Each piece is styled through
/// `Theme::scope_style` (never `overlay_scope_style`: a decoration cell has
/// no base cell underneath it to preserve a background for, unlike an
/// overlay patch onto already-emitted content).
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
                buf_offset: -1,
            });
        }
    }
    cells
}

/// `row`'s own decoration width in terminal cells — `0` for an undecorated
/// row. The single source `apply_cursor_overlays` (shifting a caret's
/// `visual_col` past the decor prefix), `commands::mouse::offset_at`
/// (subtracting the same prefix before its cell walk) and the code-region
/// background fill (starting at the first content column) all read, so none
/// of them can disagree about how wide a given row's decoration rendered.
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
    fn decor_cells_carry_buf_offset_negative_one_and_the_pieces_style() {
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
            assert_eq!(cell.buf_offset, -1);
            assert_eq!(cell.style, theme.scope_style(scope));
        }
    }
}
