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
}

/// A table source line's row 1 is `line.spans` (already tiled by the table
/// renderer) and never re-wrapped — Grid layout leaves `extra_rows` empty,
/// so this pushes exactly one segment; Wrapped/Pivoted push one more per
/// `extra_rows` entry, each `start_col`ed at the running sum of the
/// previous rows' own visible lengths, so `syntax_to_wrap`/`wrap_to_syntax`
/// — purely mechanical over `start_col` + a segment's own visible length —
/// round-trip a multi-row table line with zero special-casing.
pub(super) fn wrap_table_line(
    line_idx: usize,
    line: &SyntaxLine,
    info: &TableRowInfo,
    segments: &mut Vec<WrapSegment>,
) {
    let seg_info = TableSegInfo {
        col_widths: info.col_widths.clone(),
        role: info.role,
        boundary: info.boundary,
    };
    segments.push(WrapSegment {
        spans: line.spans.clone(),
        model_line: line_idx,
        start_col: 0,
        table: Some(seg_info.clone()),
    });
    let mut start_col: usize = line.spans.iter().map(query::span_visible_len).sum();
    for extra in &info.extra_rows {
        segments.push(WrapSegment {
            spans: extra.clone(),
            model_line: line_idx,
            start_col,
            table: Some(seg_info.clone()),
        });
        start_col += extra.iter().map(query::span_visible_len).sum::<usize>();
    }
}
