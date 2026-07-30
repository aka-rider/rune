//! [rune-syntax 6]: `WrapSnapshot`'s coordinate-query edges — the domain
//! `wrap/query.rs` (216 lines at review time) shipped with ZERO in-crate
//! tests of its own, validated only indirectly through `rune-md`/
//! `rune-fuzz`. Covers: an empty line, width 0, a full-width (CJK) char
//! sitting exactly at the wrap column, and a row/column past the last row.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use rune_core::coords::{SyntaxPoint, WrapPoint};
use rune_syntax::scope::ScopeId;
use rune_syntax::syntax::{SyntaxLine, SyntaxSpan};
use rune_syntax::wrap::WrapMap;

const SCOPE: ScopeId = ScopeId(0);

fn one_span_line(content: &str) -> SyntaxLine {
    SyntaxLine {
        spans: vec![SyntaxSpan::identical(content, SCOPE, 0..content.len())],
        table: None,
        decor: None,
    }
}

#[test]
fn empty_line_produces_a_single_zero_length_segment() {
    let content = "";
    let wrap = WrapMap::new(40).sync(content, &[SyntaxLine::default()]);
    assert_eq!(wrap.total_rows(), 1);
    assert_eq!(wrap.segment_len_at(0), 0);
    // Round-trips to itself: there's nowhere else for col 0 to go.
    let wp = wrap.syntax_to_wrap(SyntaxPoint { line: 0, col: 0 });
    assert_eq!(wp, WrapPoint { row: 0, col: 0 });
    let sp = wrap.wrap_to_syntax(content, wp);
    assert_eq!(sp, SyntaxPoint { line: 0, col: 0 });
}

#[test]
fn width_zero_never_wraps_pushes_the_whole_line_as_one_segment() {
    let content = "this line would wrap at any real width but not at zero";
    let line = one_span_line(content);
    let wrap = WrapMap::new(0).sync(content, &[line]);
    assert_eq!(wrap.total_rows(), 1);
    assert_eq!(wrap.segment_len_at(0), content.len());
}

#[test]
fn full_width_cjk_char_landing_exactly_at_the_wrap_column_does_not_split() {
    // Each CJK char is 2 cells wide; width 4 fits exactly two of them with
    // no remainder — the greedy breaker must not straddle a cluster.
    let content = "\u{4e2d}\u{6587}\u{4e2d}\u{6587}"; // 中文中文, 4 chars, 8 cells total
    let line = one_span_line(content);
    let wrap = WrapMap::new(4).sync(content, &[line]);
    // Two segments of 2 chars (6 bytes) each; every segment's own text is a
    // whole number of CJK chars, never half of one (which would be
    // impossible anyway — UTF-8 chars are indivisible bytes — but a
    // boundary landing mid-cluster silently mismeasures visual width).
    assert_eq!(wrap.total_rows(), 2);
    for row in 0..2 {
        let visual = wrap.visual_col(content, row, wrap.segment_len_at(row));
        assert_eq!(visual, 4, "row {row} must fill exactly the 4-cell width");
    }
}

#[test]
fn segment_len_at_past_the_last_row_clamps_instead_of_panicking() {
    let content = "one line only\n";
    let line = one_span_line(content);
    let wrap = WrapMap::new(80).sync(content, &[line]);
    assert_eq!(wrap.total_rows(), 1);
    // A row index far past the last one clamps to the last row rather than
    // returning 0 or panicking — `clamp_row`'s contract.
    assert_eq!(wrap.segment_len_at(999), wrap.segment_len_at(0));
}

#[test]
fn wrap_to_syntax_past_the_last_row_clamps_to_the_last_rows_own_end() {
    let content = "abc";
    let line = one_span_line(content);
    let wrap = WrapMap::new(80).sync(content, &[line]);
    let sp = wrap.wrap_to_syntax(content, WrapPoint { row: 999, col: 999 });
    // Clamped to the (only) real row, at its own end.
    assert_eq!(sp, SyntaxPoint { line: 0, col: 3 });
}

#[test]
fn syntax_to_wrap_past_the_last_line_clamps_to_the_last_line() {
    let content = "only one line";
    let line = one_span_line(content);
    let wrap = WrapMap::new(80).sync(content, &[line]);
    let wp = wrap.syntax_to_wrap(SyntaxPoint { line: 999, col: 0 });
    assert_eq!(wp.row, 0);
}
