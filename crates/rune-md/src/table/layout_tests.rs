//! Tests for the Grid geometry helpers, split out to keep the owning
//! module under the 500-line budget.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;

fn rendered(text: &str, start_buf: u32, scope: ScopeId) -> RenderedCell {
    let src = (0..text.chars().count() as u32)
        .map(|i| CellSrc {
            buf: Some(start_buf + i),
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

/// Center alignment's fill split is `(fill / 2, fill - fill / 2)` — the
/// shorter half goes on the LEFT, the longer half on the right, for an ODD
/// fill (an even fill splits evenly either way and can't tell the two
/// halves apart). Content "ab" (width 2) in a width-5 column leaves fill 3:
/// left = 1, right = 2.
#[test]
fn center_alignment_gives_the_extra_odd_space_to_the_right() {
    let widths = vec![5];
    let runs = grid_row(
        &widths,
        &[TableAlign::Center],
        &[rendered("ab", 0, ScopeId(1))],
        ScopeId(1),
    );
    let text: String = runs.iter().map(|(t, _, _)| t.as_str()).collect();
    assert_eq!(text, "│  ab   │");
}

/// `choose`'s Grid-fit test at exactly the boundary: a table whose true
/// rendered width (`Σw + 3n + 1` — the same formula this test pins) is
/// exactly `avail` still fits Grid; one column narrower than that and it
/// doesn't.
#[test]
fn choose_picks_grid_exactly_at_the_true_rendered_width_boundary() {
    assert_eq!(choose(&[3, 3], &[1, 1], 13), TableLayout::Grid);
    assert_eq!(choose(&[3, 3], &[1, 1], 12), TableLayout::Pivoted);
}

/// `choose`'s frame overhead (`4n - 1`) and its derived `content_budget`
/// (`avail - frame_overhead`) and `equal_share` (`content_budget / n`)
/// all feed the atomic/flex split: get any of the three arithmetic
/// operators wrong here and both columns (min width 5, comfortably under
/// any of the wrong `equal_share`s) end up misclassified as atomic and
/// over budget, picking Wrapped where the real formula picks Pivoted.
#[test]
fn choose_frame_overhead_and_content_budget_use_the_right_operators() {
    assert_eq!(choose(&[20, 20], &[5, 5], 30), TableLayout::Pivoted);
}

/// A second, independent pin for `equal_share = content_budget / n`: with
/// `/` replaced by `*`, `equal_share` balloons so large that a column
/// which should stay atomic (min width 14, above the true equal_share of
/// 12) gets reclassified as flexible, changing the layout Pivoted picks.
#[test]
fn choose_equal_share_is_a_division_not_a_multiplication() {
    assert_eq!(choose(&[20, 20], &[14, 1], 32), TableLayout::Pivoted);
}

/// The atomic/flex boundary in `choose` is `min_w > equal_share`, not
/// `==`/`>=`/`<`: a column sitting EXACTLY at `equal_share` (11, here)
/// must count as flexible, same as `>`'s strict reading — `==`/`>=` would
/// wrongly promote it to atomic, and once atomic its own width plus the
/// genuinely-atomic column's (12) exceeds the 35-wide budget, flipping
/// this table's own Pivoted result to Wrapped.
#[test]
fn choose_atomic_boundary_column_at_equal_share_stays_flexible() {
    assert_eq!(
        choose(&[15, 15, 15], &[12, 11, 1], 46),
        TableLayout::Pivoted
    );
}

/// The same boundary from the other side: a column strictly ABOVE
/// `equal_share` (13, `equal_share` is 12 here) must count as atomic —
/// `<` would wrongly demote it to flexible, which (combined with the
/// genuinely-flexible second column) gives this table enough apparent
/// flex room to pick Wrapped instead of the real Pivoted result.
#[test]
fn choose_atomic_boundary_column_above_equal_share_stays_atomic() {
    assert_eq!(choose(&[20, 20], &[13, 1], 31), TableLayout::Pivoted);
}

/// `flex_cols_have_room`'s threshold is `flex_count * min_flex`, not
/// `flex_count + min_flex` or `flex_count / min_flex` — both wrong
/// operators shrink the threshold enough that this table's genuinely
/// insufficient budget (15, needs 24) reads as sufficient.
#[test]
fn choose_flex_threshold_multiplies_flex_count_by_min_flex() {
    assert_eq!(choose(&[10, 10], &[5, 5], 22), TableLayout::Pivoted);
}

/// `constrain_widths`'s `remaining = content_budget - floor_total` (not
/// `/`) and its `if remaining <= 0` early return (not `>`): with budget
/// (20) comfortably above the floor total (6), both columns should
/// stretch all the way to their natural width.
#[test]
fn constrain_widths_stretches_columns_to_natural_width_when_budget_allows() {
    assert_eq!(constrain_widths(&[10, 10], &[3, 3], 20), vec![10, 10]);
}

/// When every column is already AT its floor (natural width equals the
/// 3-wide minimum), `total_stretch` is 0 and the remaining budget (4)
/// must split evenly via `remaining / n` (not `%`, not `*`) and land via
/// `floor + per_col` (not `-`, not `*`).
#[test]
fn constrain_widths_splits_leftover_evenly_when_no_column_can_stretch() {
    assert_eq!(constrain_widths(&[3, 3], &[3, 3], 10), vec![5, 5]);
}

/// The proportional-stretch path exercises every remaining operator at
/// once: `(stretch * remaining) / total_stretch` (not `stretch +
/// remaining`, `stretch / remaining`, or `.../ total_stretch` replaced by
/// `%`), the final leftover-to-widest-column top-up gated on `leftover >
/// 0` (not `==`/`<`), and that top-up applied via `+=` (not `-=`/`*=`).
/// Column 0 (natural 10, floor 3) stretches to 6; column 1 (natural 20,
/// floor 3, the widest) stretches to 11 and then picks up the leftover
/// unit of budget the proportional split couldn't place, landing at 12.
#[test]
fn constrain_widths_distributes_proportionally_and_gives_the_leftover_to_the_widest_column() {
    assert_eq!(constrain_widths(&[10, 20], &[3, 3], 18), vec![6, 12]);
}
