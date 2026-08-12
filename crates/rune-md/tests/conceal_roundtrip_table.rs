//! Split off `conceal_roundtrip.rs` (WP11): MAJOR (verification
//! round 9) — `Block::Verbatim` (GFM tables, HTML blocks, and unknown/math
//! constructs) never received the container-aware per-line treatment
//! `CodeFenceM`/`HeadingM`/`HrM` got across rounds 4-7 — the dominant
//! remaining panic class under the adopted reviewer alphabet (~325/214k).
//! Minimal repro: "> t\n> ---|" (comrak recognizes "t\n---|" as a single-
//! column GFM table; the un-clamped `range` used to re-claim the
//! blockquote's own "> " marker on the table's second line). Fixed the
//! same way `CodeFenceM` was: `VerbatimM::content_lines`, one `ByteRange`
//! per COMRAK line, built at parse time via the shared `per_line_content`
//! chokepoint. Assertions check the EXACT rendered text (code/quote content
//! must appear in the output, not just pass a coverage check) — a
//! coverage-only check can't tell "byte accounted for" from "byte
//! accounted for AND shown".
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod conceal_common;
mod table_render_common;

use conceal_common::{
    assert_no_duplicate_content, assert_no_duplicate_content_at, joined_line, synced,
};
use rune_core::coords::BufferPoint;
use rune_md::emit::emit;
use table_render_common::wrap_pivot_url;

#[test]
fn table_in_blockquote_does_not_double_claim() {
    let content = "> t\n> ---|";
    assert_no_duplicate_content(content);
    // Cursor sits at buffer offset 0, on the table's own first line, so the
    // whole table reveals as a unit when focused (plan architectural
    // decision 5) — the delimiter line stays raw markdown, "---|", exactly
    // as `synced(content, 0, true)` puts the cursor inside its range.
    let (buf, doc) = synced(content, 0, true);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    let joined = joined_line(&lines, 1, buf.content());
    assert!(
        joined.contains("---|"),
        "table separator row missing from revealed output: {joined:?}"
    );
    // Unfocused forces every Decide-policy block Rendered regardless of
    // cursor position — a real Grid layout (WP2) now replaces the raw
    // delimiter line with a box-drawn separator row instead of leaving it
    // as verbatim markdown (the pre-WP2 scaffold's behaviour, which this
    // test used to pin).
    let (buf, doc) = synced(content, 0, false);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    let joined = joined_line(&lines, 1, buf.content());
    assert!(
        joined.contains('├') && joined.contains('┤') && !joined.contains("---|"),
        "expected a Grid separator row when unfocused, got: {joined:?}"
    );
}

#[test]
fn table_in_nested_blockquote_and_list_item_does_not_double_claim() {
    assert_no_duplicate_content("> > t\n> > ---|");
    assert_no_duplicate_content("- t\n  ---|");
    assert_no_duplicate_content("1. t\n   ---|");
}

#[test]
fn table_control_without_trailing_pipe_stays_clean() {
    // The control: "t\n---" (no "|") is a setext heading, not a table —
    // must stay clean either way, already covered by round 5-7's fix,
    // pinned here again as the direct control for the table repro above.
    assert_no_duplicate_content("> t\n> ---");
}

#[test]
fn html_block_in_container_does_not_lose_content() {
    let content = "> <div\n> foo>text";
    assert_no_duplicate_content(content);
    for &focused in &[true, false] {
        let (buf, doc) = synced(content, 0, focused);
        let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
        let joined = joined_line(&lines, 1, buf.content());
        assert!(
            joined.contains("foo"),
            "HTML block content missing from rendered output (focused={focused}): {joined:?}"
        );
    }
    assert_no_duplicate_content("- <div\n  foo>text");
}

#[test]
fn table_and_html_block_in_container_cr_variants_stay_clean() {
    assert_no_duplicate_content("> t\r> ---|");
    assert_no_duplicate_content("- t\r  ---|");
    assert_no_duplicate_content("> <div\r> foo>text");
}

#[test]
fn pivoted_table_accounts_every_byte_of_its_suppressed_rows() {
    assert_no_duplicate_content_at(&wrap_pivot_url(), &[0], 20);
}

#[test]
fn pivoted_table_suppressed_header_line_hides_fully_and_clamps_stably() {
    let content = wrap_pivot_url();
    let (buf, doc) = synced(&content, 0, false);
    let width = 20u16;
    let (_lines, snap) = emit(buf.content(), doc.blocks(), width);

    let line = 0;
    let line_len = buf.line(line).len();
    assert_eq!(snap.hidden_byte_count(line), line_len);

    for col in 0..=line_len {
        let bp = BufferPoint { line, col };
        let sp = snap.buffer_to_syntax(bp);
        let bp2 = snap.syntax_to_buffer(sp);
        let sp2 = snap.buffer_to_syntax(bp2);
        assert_eq!(
            sp, sp2,
            "clamped position must be stable under a second round-trip, col {col}"
        );
    }
}
