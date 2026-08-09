//! Tests for the Grid geometry helpers, split out to keep the owning
//! module under the 500-line budget.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;

fn rendered(text: &str, start_buf: i64, scope: ScopeId) -> RenderedCell {
    let src = (0..text.chars().count() as i64)
        .map(|i| CellSrc {
            buf: start_buf + i,
            scope,
        })
        .collect();
    RenderedCell {
        text: text.to_string(),
        src,
    }
}

#[test]
fn grid_row_pads_left_aligned_content_to_column_width() {
    let widths = vec![5, 3];
    let aligns = vec![TableAlign::None, TableAlign::None];
    let cells = vec![
        rendered("Name", 2, ScopeId(9)),
        rendered("Age", 9, ScopeId(9)),
    ];
    let runs = grid_row(&widths, &aligns, &cells, ScopeId(9));
    let text: String = runs.iter().map(|(t, _, _)| t.as_str()).collect();
    assert_eq!(text, "│ Name  │ Age │");
}

/// A grapheme cluster's CELL column, not its char index — alignment is
/// a display-width property (a CJK cell's char count legitimately
/// differs from its cell width, WP9.S3/[rune-md 6]), so both the total
/// width comparison and the bar/corner position comparison below walk
/// `graphemes(true)` and accumulate `grapheme_width`, never `.chars()`.
fn cell_cols_of(text: &str, matches: impl Fn(&str) -> bool) -> Vec<usize> {
    let mut col = 0usize;
    let mut out = Vec::new();
    for g in text.graphemes(true) {
        if matches(g) {
            out.push(col);
        }
        col += grapheme_width(g);
    }
    out
}

#[test]
fn separator_row_matches_grid_rows_total_width() {
    let widths = vec![5, 3];
    let grid_text: String = grid_row(
        &widths,
        &[TableAlign::None, TableAlign::None],
        &[],
        ScopeId(1),
    )
    .iter()
    .map(|(t, _, _)| t.as_str())
    .collect::<String>();
    let sep = separator_row(&widths);
    let sep_text = &sep[0].0;
    assert_eq!(display_width(sep_text), display_width(&grid_text));
    // Bars/corners must land at the SAME visual COLUMN (display cells)
    // in both rows, not the same char index.
    let bar_cols = cell_cols_of(&grid_text, |g| g == "│");
    let corner_cols = cell_cols_of(sep_text, |g| matches!(g, "├" | "┼" | "┤"));
    assert_eq!(bar_cols, corner_cols);
}
