//! [rune-syntax 6]: `wrap_table_line`'s row-boundary matrix, exercised only
//! through the public `WrapMap::sync` entry point (the function itself is
//! `pub(super)`, reached only by producing a `SyntaxLine` whose `table` is
//! `Some` — exactly what a real table-producing emitter hands the wrap
//! pass). Before this file, `cargo test -p rune-syntax` passed with this
//! logic broken — its regression coverage lived entirely in
//! `rune-md/tests/table_render.rs`, which only exercises Grid layout (a
//! single segment per line), never the Wrapped/Pivoted multi-segment case
//! this matrix pins.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use rune_syntax::scope::ScopeId;
use rune_syntax::syntax::{RowBoundary, SyntaxLine, SyntaxSpan, TableRole, TableRowInfo};
use rune_syntax::wrap::WrapMap;

const SCOPE: ScopeId = ScopeId(0);

/// One 4-byte-wide `Identical` span, used as filler for every row so
/// `start_col` accumulation has a known, non-zero visible length to check
/// against.
fn filler_span(content: &str, range: std::ops::Range<usize>) -> Vec<SyntaxSpan> {
    vec![SyntaxSpan::identical(content, SCOPE, range)]
}

fn table_line(content: &str, boundary: RowBoundary, extra_rows: usize) -> SyntaxLine {
    let row1 = filler_span(content, 0..4);
    let extra: Vec<Vec<SyntaxSpan>> = (0..extra_rows)
        .map(|i| filler_span(content, (4 + i * 4)..(8 + i * 4)))
        .collect();
    SyntaxLine {
        spans: row1,
        table: Some(TableRowInfo {
            col_widths: vec![4],
            role: TableRole::Body,
            boundary,
            extra_rows: extra,
            boxed: true,
        }),
    }
}

fn boundaries_for(boundary: RowBoundary, extra_rows: usize) -> Vec<RowBoundary> {
    // 4 bytes per row is enough content for `extra_rows` extra segments plus
    // row 1; the exact text doesn't matter, only that each row's filler span
    // has a distinct byte range so `start_col` accumulation is checkable.
    let content = "x".repeat(4 + extra_rows * 4);
    let line = table_line(&content, boundary, extra_rows);
    let width = 999; // wide enough that only the table branch ever fires
    let wrap = WrapMap::new(width).sync(&content, &[line]);
    wrap.segments()
        .iter()
        .map(|seg| {
            seg.table
                .as_ref()
                .map(|t| t.boundary)
                .expect("every segment of a table source line carries TableSegInfo")
        })
        .collect()
}

#[test]
fn only_boundary_with_no_extra_rows_stays_only() {
    assert_eq!(
        boundaries_for(RowBoundary::Only, 0),
        vec![RowBoundary::Only]
    );
}

#[test]
fn first_boundary_with_no_extra_rows_becomes_first_alone() {
    // Grid layout (`extra_rows` empty) never produces more than one
    // segment, so a `First` boundary with nothing following it still only
    // ever gets ONE segment carrying it — the source SyntaxLine's boundary
    // classification, not `wrap_table_line`'s internal seam logic, is what
    // decides whether that's actually correct table geometry.
    assert_eq!(
        boundaries_for(RowBoundary::First, 0),
        vec![RowBoundary::First]
    );
}

#[test]
fn middle_boundary_with_no_extra_rows_stays_middle() {
    assert_eq!(
        boundaries_for(RowBoundary::Middle, 0),
        vec![RowBoundary::Middle]
    );
}

#[test]
fn last_boundary_with_no_extra_rows_stays_last() {
    assert_eq!(
        boundaries_for(RowBoundary::Last, 0),
        vec![RowBoundary::Last]
    );
}

#[test]
fn only_boundary_with_extra_rows_brackets_first_and_last_only() {
    // A Pivoted/Wrapped table line spanning 3 visual rows: the top border
    // goes before the FIRST segment, the bottom border after the LAST — no
    // border may ever land between two segments of the SAME source line.
    assert_eq!(
        boundaries_for(RowBoundary::Only, 2),
        vec![RowBoundary::First, RowBoundary::Middle, RowBoundary::Last]
    );
}

#[test]
fn first_boundary_with_extra_rows_never_lets_a_later_segment_claim_first() {
    assert_eq!(
        boundaries_for(RowBoundary::First, 2),
        vec![RowBoundary::First, RowBoundary::Middle, RowBoundary::Middle]
    );
}

#[test]
fn last_boundary_with_extra_rows_never_lets_an_earlier_segment_claim_last() {
    assert_eq!(
        boundaries_for(RowBoundary::Last, 2),
        vec![RowBoundary::Middle, RowBoundary::Middle, RowBoundary::Last]
    );
}

#[test]
fn middle_boundary_with_extra_rows_never_synthesises_a_border_anywhere() {
    assert_eq!(
        boundaries_for(RowBoundary::Middle, 2),
        vec![
            RowBoundary::Middle,
            RowBoundary::Middle,
            RowBoundary::Middle
        ]
    );
}

#[test]
fn extra_row_start_col_accumulates_the_previous_rows_own_visible_length() {
    let content = "x".repeat(12); // row1 (4) + 2 extra rows (4 each)
    let line = table_line(&content, RowBoundary::Only, 2);
    let wrap = WrapMap::new(999).sync(&content, &[line]);
    let segs = wrap.segments();
    assert_eq!(segs.len(), 3);
    assert_eq!(segs[0].start_col, 0);
    assert_eq!(segs[1].start_col, 4);
    assert_eq!(segs[2].start_col, 8);
}

#[test]
fn table_seg_info_carries_col_widths_role_and_boxed_unchanged() {
    let content = "xxxx";
    let line = table_line(content, RowBoundary::Only, 0);
    let wrap = WrapMap::new(999).sync(content, &[line]);
    let info = wrap.segments()[0].table.as_ref().expect("table segment");
    assert_eq!(info.col_widths, vec![4]);
    assert_eq!(info.role, TableRole::Body);
    assert!(info.boxed);
}
