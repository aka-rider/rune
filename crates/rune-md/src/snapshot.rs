//! `DisplaySnapshot` (plan Context, "Emit -> wrap -> snapshot"): the wrap
//! rows, expanded with the ONE thing wrap genuinely cannot do — synthesised
//! top/bottom/inter-row table borders that have no source line at all
//! (architectural decision 3). `from_wrap` is the identity (one
//! `DisplayRow` per wrap row); `expand_tables` walks a table's rendered
//! rows and inserts a synthetic border row wherever `TableSegInfo::boundary`
//! says one belongs. Kept as its own type, distinct from `WrapSnapshot`, so
//! every display-space consumer (`render::build_rows`, the viewport, mouse
//! hit-testing) reads row geometry through ONE place instead of each
//! re-deriving "which wrap row is this synthetic border next to" itself.

use rune_syntax::SyntaxSpan;
use rune_syntax::syntax::{RowBoundary, TableRole};
use rune_syntax::wrap::WrapSnapshot;

use crate::emit::style::table_border_scope;
use crate::table::layout::{BorderKind, border_row};

/// One row of the display grid: either a real wrap row's own spans
/// (`synthetic: false`, `wrap_row` its own row number), or a synthesised
/// table border with no source line at all (`synthetic: true`, `wrap_row`
/// borrowed from the adjacent content row it was inserted next to — see
/// `expand_tables`). Every char of a synthetic row's one span carries
/// `cell_map` entry `-1` and an empty `range`: it has no buffer
/// correspondence, decorative through and through (plan Gotcha 3, the
/// `CELL-OFFSET` fuzz invariant).
#[derive(Clone, Debug)]
pub struct DisplayRow {
    pub spans: Vec<SyntaxSpan>,
    pub wrap_row: usize,
    pub synthetic: bool,
}

#[derive(Clone, Debug, Default)]
pub struct DisplaySnapshot {
    rows: Vec<DisplayRow>,
    /// `wrap_to_display[w]` is the display-row index of wrap row `w`'s OWN
    /// (non-synthetic) `DisplayRow` — sized to `wrap.total_rows()`, one
    /// entry per wrap row, populated only by `expand_tables` (`from_wrap`
    /// alone is already the identity map).
    wrap_to_display: Vec<usize>,
}

impl DisplaySnapshot {
    /// The identity mapping: one `DisplayRow` per wrap row, none synthetic.
    pub fn from_wrap(wrap: &WrapSnapshot) -> DisplaySnapshot {
        let rows: Vec<DisplayRow> = wrap
            .segments()
            .iter()
            .enumerate()
            .map(|(i, seg)| DisplayRow {
                spans: seg.spans.clone(),
                wrap_row: i,
                synthetic: false,
            })
            .collect();
        let wrap_to_display = (0..rows.len()).collect();
        DisplaySnapshot {
            rows,
            wrap_to_display,
        }
    }

    /// Inserts a synthesised border row wherever a table's own
    /// `TableSegInfo::boundary` says one belongs: `┌┬┐` before a
    /// `First`/`Only` row, `└┴┘` after a `Last`/`Only` row, and `├┼┤`
    /// between two consecutive `Body` rows that come from DIFFERENT source
    /// lines (never between two visual rows of the SAME wrapped/pivoted
    /// line — the `model_line` check below is what tells them apart; a
    /// header's own separator line already supplies its own real content
    /// row, so no synthetic border is ever needed between Header and
    /// Separator or Separator and the first Body row).
    ///
    /// A leading (`Top`/`Middle`) border borrows the wrap_row/line-start of
    /// the row it precedes — the one that follows it, which always exists;
    /// a trailing (`Bottom`) border borrows the row it follows — the one
    /// that precedes it, since nothing inside the table follows it. Every
    /// synthetic row's `wrap_row` is therefore always an adjacent content
    /// row's own `wrap_row`.
    pub fn expand_tables(self, wrap: &WrapSnapshot) -> DisplaySnapshot {
        let segments = wrap.segments();
        let mut rows: Vec<DisplayRow> = Vec::with_capacity(self.rows.len());
        let mut wrap_to_display = vec![0usize; segments.len()];
        // (role, model_line) of the previous row that carried table info —
        // `None` once a non-table row (or the very start) breaks the run.
        let mut prev_table: Option<(TableRole, usize)> = None;

        for row in self.rows {
            let seg = segments.get(row.wrap_row);
            let info = seg.and_then(|s| s.table.as_ref());
            let model_line = seg.map(|s| s.model_line).unwrap_or(0);
            let line_start = line_start_of(&row);

            if let Some(info) = info {
                let starts_table = matches!(info.boundary, RowBoundary::First | RowBoundary::Only);
                if starts_table {
                    rows.push(synthetic_border(
                        &info.col_widths,
                        BorderKind::Top,
                        row.wrap_row,
                        line_start,
                    ));
                } else if let Some((prev_role, prev_line)) = prev_table
                    && prev_role == TableRole::Body
                    && info.role == TableRole::Body
                    && prev_line != model_line
                {
                    rows.push(synthetic_border(
                        &info.col_widths,
                        BorderKind::Middle,
                        row.wrap_row,
                        line_start,
                    ));
                }
            }

            if let Some(slot) = wrap_to_display.get_mut(row.wrap_row) {
                *slot = rows.len();
            }
            let ends_table =
                info.is_some_and(|i| matches!(i.boundary, RowBoundary::Last | RowBoundary::Only));
            let widths_for_bottom = info.map(|i| i.col_widths.clone());
            let wrap_row = row.wrap_row;
            rows.push(row);

            if ends_table && let Some(widths) = widths_for_bottom {
                rows.push(synthetic_border(
                    &widths,
                    BorderKind::Bottom,
                    wrap_row,
                    line_start,
                ));
            }

            prev_table = info.map(|i| (i.role, model_line));
        }

        DisplaySnapshot {
            rows,
            wrap_to_display,
        }
    }

    pub fn rows(&self) -> &[DisplayRow] {
        &self.rows
    }

    pub fn total_rows(&self) -> usize {
        self.rows.len()
    }

    /// The wrap row a display row was built from — for a synthetic row,
    /// the adjacent content row's own wrap row (never its own, since it has
    /// none), per `expand_tables`'s docs.
    pub fn display_to_wrap(&self, row: usize) -> usize {
        if self.rows.is_empty() {
            return 0;
        }
        let row = row.min(self.rows.len() - 1);
        self.rows.get(row).map(|r| r.wrap_row).unwrap_or(0)
    }

    /// The display row a wrap row's OWN content lives at — the inverse of
    /// `display_to_wrap` for a REAL (non-synthetic) row. Every caret/scroll
    /// computation that starts from a wrap-space coordinate (cursors always
    /// do — border rows aren't addressable) must convert through this
    /// before indexing `rows()`.
    pub fn wrap_to_display(&self, row: usize) -> usize {
        if self.wrap_to_display.is_empty() {
            return 0;
        }
        let row = row.min(self.wrap_to_display.len() - 1);
        self.wrap_to_display.get(row).copied().unwrap_or(0)
    }
}

/// A content row's own spans always tile its whole source line (Gotcha 1),
/// so its first span's range start IS the line's start — the anchor a
/// synthetic border borrows for its own (empty) range.
fn line_start_of(row: &DisplayRow) -> usize {
    row.spans.first().map(|s| s.range().start).unwrap_or(0)
}

fn synthetic_border(
    widths: &[usize],
    kind: BorderKind,
    wrap_row: usize,
    line_start: usize,
) -> DisplayRow {
    let text = border_row(widths, kind);
    let cell_map = vec![-1i64; text.chars().count()];
    let span = SyntaxSpan::Substituted {
        scope: table_border_scope(),
        text,
        range: line_start..line_start,
        cell_map,
    };
    DisplayRow {
        spans: vec![span],
        wrap_row,
        synthetic: true,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use rune_syntax::SyntaxLine;
    use rune_syntax::wrap::WrapMap;

    #[test]
    fn display_snapshot_row_count_matches_wrap_in_the_no_table_case() {
        // Two empty lines wrap to exactly 2 rows (each empty `SyntaxLine`
        // gets its own single, empty segment — see `WrapMap::wrap_line`'s
        // `line.spans.is_empty()` case). Pin that concrete count instead of
        // comparing `DisplaySnapshot::from_wrap`'s output back to
        // `wrap.total_rows()` — the very value it copies — which passes
        // trivially by construction regardless of what `total_rows()`
        // actually is.
        let lines = vec![SyntaxLine::default(), SyntaxLine::default()];
        let wrap = WrapMap::new(80).sync("", &lines);
        assert_eq!(wrap.total_rows(), 2);
        let display = DisplaySnapshot::from_wrap(&wrap);
        assert_eq!(display.total_rows(), 2);
        // No table lines: `expand_tables` must be a true no-op.
        let expanded = DisplaySnapshot::from_wrap(&wrap).expand_tables(&wrap);
        assert_eq!(expanded.total_rows(), 2);
        for r in expanded.rows() {
            assert!(!r.synthetic);
        }
    }

    /// WP3.S7: a 4-line table (header, separator, two body rows — no
    /// trailing blank line, so wrap has exactly 4 rows, one per source
    /// line) expands to 7 display rows: a synthesised `┌┬┐` before the
    /// header, the header/separator/two body rows unchanged, a synthesised
    /// `├┼┤` between the two body rows (they're different source lines,
    /// both `Body` — the case with no real separator line of their own),
    /// and a synthesised `└┴┘` after the last body row.
    #[test]
    fn four_line_table_expands_to_seven_display_rows() {
        let content = "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 25 |";
        let blocks = crate::parse::parse(content);
        let (lines, _syntax) = crate::emit::emit(content, &blocks, 80);
        assert_eq!(lines.len(), 4, "fixture must have no trailing blank line");
        let wrap = WrapMap::new(80).sync(content, &lines);
        assert_eq!(wrap.total_rows(), 4);

        let display = DisplaySnapshot::from_wrap(&wrap).expand_tables(&wrap);
        assert_eq!(display.total_rows(), 7);

        let synthetic_count = display.rows().iter().filter(|r| r.synthetic).count();
        assert_eq!(synthetic_count, 3, "top + bottom + one inter-row border");

        assert!(display.rows().first().is_some_and(|r| r.synthetic));
        assert!(display.rows().last().is_some_and(|r| r.synthetic));
    }

    /// `display_to_wrap(wrap_to_display(r)) == r` for every wrap row, and
    /// every synthetic row's `wrap_row` equals an adjacent content row's own
    /// — the WP3.S7 round-trip and adjacency assertions.
    #[test]
    fn wrap_display_round_trip_and_synthetic_adjacency() {
        let content = "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 25 |";
        let blocks = crate::parse::parse(content);
        let (lines, _syntax) = crate::emit::emit(content, &blocks, 80);
        let wrap = WrapMap::new(80).sync(content, &lines);
        let display = DisplaySnapshot::from_wrap(&wrap).expand_tables(&wrap);

        for w in 0..wrap.total_rows() {
            let d = display.wrap_to_display(w);
            assert_eq!(
                display.display_to_wrap(d),
                w,
                "round trip failed for wrap row {w}"
            );
            assert!(
                !display.rows()[d].synthetic,
                "wrap_to_display must land on a real row"
            );
        }

        for (i, row) in display.rows().iter().enumerate() {
            if !row.synthetic {
                continue;
            }
            let prev = i.checked_sub(1).and_then(|j| display.rows().get(j));
            let next = display.rows().get(i + 1);
            assert!(
                prev.is_some_and(|r| r.wrap_row == row.wrap_row)
                    || next.is_some_and(|r| r.wrap_row == row.wrap_row),
                "synthetic row {i}'s wrap_row must equal an adjacent content row's"
            );
        }
    }
}
