//! The wrap pass's table branch: a table source line arrives already laid
//! out to the available width, so it is never re-wrapped — it is projected
//! straight into one segment per visual row.
//!
//! Kept out of the wrap pass's own module so that module stays under the
//! §1.6 size budget, and because "a pre-laid-out line bypasses greedy
//! breaking entirely" is a distinct concern from the greedy breaker itself.

use super::WrapSegment;
use super::query;
use crate::syntax::{RowBoundary, SyntaxLine, TableRole, TableRowInfo};

/// A table `WrapSegment`'s own geometry — the `WrapSegment`-level
/// projection of `SyntaxLine`'s `TableRowInfo` (`col_widths`/`role` carried
/// unchanged; `boundary` too, since Grid never splits one source line into
/// more than one row-1 segment — Wrapped/Pivoted layout is what makes a
/// single source line's `extra_rows` produce more than one segment). The
/// display pass reads this to decide where to synthesize a top/bottom/
/// inter-row border row.
#[derive(Clone, Debug)]
pub struct TableSegInfo {
    pub col_widths: Vec<usize>,
    pub role: TableRole,
    pub boundary: RowBoundary,
    /// Carried through from the source line: a Pivoted table draws no box,
    /// so no border rows may be synthesised around it.
    pub boxed: bool,
}

/// A table source line's row 1 is `line.spans` (already tiled by the table
/// renderer) and never re-wrapped — Grid layout leaves `extra_rows` empty,
/// so this pushes exactly one segment; Wrapped/Pivoted push one more per
/// `extra_rows` entry, each `start_col`ed at the running sum of the
/// previous rows' own visible lengths, so `syntax_to_wrap`/`wrap_to_syntax`
/// — purely mechanical over `start_col` + a segment's own visible length —
/// round-trip a multi-row table line with zero special-casing.
///
/// `info.boundary` describes the LOGICAL row's own boundary (whether a top
/// and/or bottom border belongs around it) — a property of the source line
/// as a whole, not of any one of its visual sub-rows. Only the FIRST
/// segment may ever carry a `First`/`Only` boundary (the top border goes
/// before it) and only the LAST segment may ever carry a `Last`/`Only`
/// boundary (the bottom border goes after it): every segment in between —
/// and, for a Grid line, there is exactly one segment so this never fires —
/// is forced to `Middle`, so `DisplaySnapshot::expand_tables` never
/// synthesises a border between two visual rows of the SAME wrapped line.
pub(super) fn wrap_table_line(
    line_idx: usize,
    line: &SyntaxLine,
    info: &TableRowInfo,
    segments: &mut Vec<WrapSegment>,
) {
    let total_segments = 1 + info.extra_rows.len();
    let starts = matches!(info.boundary, RowBoundary::First | RowBoundary::Only);
    let ends = matches!(info.boundary, RowBoundary::Last | RowBoundary::Only);
    let seg_boundary = |i: usize| -> RowBoundary {
        let is_first = i == 0;
        let is_last = i == total_segments - 1;
        match (starts && is_first, ends && is_last) {
            (true, true) => RowBoundary::Only,
            (true, false) => RowBoundary::First,
            (false, true) => RowBoundary::Last,
            (false, false) => RowBoundary::Middle,
        }
    };

    segments.push(WrapSegment {
        spans: line.spans.clone(),
        model_line: line_idx,
        start_col: 0,
        table: Some(TableSegInfo {
            col_widths: info.col_widths.clone(),
            role: info.role,
            boundary: seg_boundary(0),
            boxed: info.boxed,
        }),
        // A table source line is never decorated (WP2.S5: "Table lines:
        // never decorated") — its own row geometry is described by `table`.
        decor: None,
    });
    let mut start_col: usize = line.spans.iter().map(query::span_visible_len).sum();
    for (k, extra) in info.extra_rows.iter().enumerate() {
        segments.push(WrapSegment {
            spans: extra.clone(),
            model_line: line_idx,
            start_col,
            table: Some(TableSegInfo {
                col_widths: info.col_widths.clone(),
                role: info.role,
                boundary: seg_boundary(k + 1),
                boxed: info.boxed,
            }),
            decor: None,
        });
        start_col += extra.iter().map(query::span_visible_len).sum::<usize>();
    }
}
