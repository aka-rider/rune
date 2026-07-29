//! [rune-syntax 6]: tab expansion — "untested by anything anywhere" at
//! review time, despite the `%4` formula being duplicated (pre-WP8) across
//! `rune_width_with_tab`/`grapheme_width_with_tab` and now unified behind
//! `TAB_STOP` (WP8.S5). Covers both public width functions plus the wrap
//! pass actually breaking a tab-containing line at the right cell.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use rune_syntax::scope::ScopeId;
use rune_syntax::syntax::{SyntaxLine, SyntaxSpan};
use rune_syntax::wrap::{TAB_STOP, WrapMap, grapheme_width_with_tab, rune_width_with_tab};

const SCOPE: ScopeId = ScopeId(0);

#[test]
fn tab_stop_is_four() {
    assert_eq!(TAB_STOP, 4);
}

#[test]
fn rune_width_with_tab_expands_to_the_next_stop() {
    assert_eq!(rune_width_with_tab('\t', 0), 4);
    assert_eq!(rune_width_with_tab('\t', 1), 3);
    assert_eq!(rune_width_with_tab('\t', 3), 1);
    assert_eq!(rune_width_with_tab('\t', 4), 4); // already on a stop: full jump
}

#[test]
fn grapheme_width_with_tab_agrees_with_the_rune_version_on_every_non_tab_input() {
    // A tab is always its own single-rune cluster (grapheme segmentation
    // never joins a control char to a neighbor), so the two width
    // functions must never drift apart on any input.
    for current in 0..8 {
        assert_eq!(
            grapheme_width_with_tab("\t", current),
            rune_width_with_tab('\t', current)
        );
        assert_eq!(
            grapheme_width_with_tab("a", current),
            rune_width_with_tab('a', current)
        );
    }
}

#[test]
fn wrap_line_expands_tabs_before_greedy_breaking_and_never_exceeds_the_width() {
    // "a\tb\tc": 'a' (1 cell), tab to col 4 (3 cells), 'b' (1 cell, col 5),
    // tab to col 8 (3 cells), 'c' (1 cell, col 9) — a byte-length-only
    // breaker would fit all 5 bytes under width 5; a cell-aware one must
    // not, since the expanded tabs push the visual width well past it.
    let content = "a\tb\tc";
    let line = SyntaxLine {
        spans: vec![SyntaxSpan::identical(content, SCOPE, 0..content.len())],
        table: None,
    };
    let width = 5u16;
    let wrap = WrapMap::new(width).sync(content, &[line]);

    assert!(
        wrap.total_rows() > 1,
        "a tab-expanded line wider than the configured width must wrap"
    );
    let mut joined = String::new();
    for row in 0..wrap.total_rows() {
        let seg = &wrap.segments()[row];
        let seg_text: String = seg.spans.iter().map(|s| s.text(content)).collect();
        let visual = wrap.visual_col(content, row, seg_text.len());
        assert!(
            visual <= width as usize,
            "row {row} ({seg_text:?}) is {visual} cells wide, over the configured width {width}"
        );
        joined.push_str(&seg_text);
    }
    // Every row's own text concatenates back to the exact original line —
    // tab expansion changes where the breaker cuts, never what bytes exist.
    assert_eq!(joined, content);
}
