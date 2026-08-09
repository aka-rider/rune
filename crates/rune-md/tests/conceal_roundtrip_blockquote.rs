//! Split off `conceal_roundtrip.rs` (WP11): blockquote-container
//! regression cases from three separate rounds sharing the same family —
//! a multi-line construct's own raw range fed whole into the generic
//! per-line splitter re-claims a REPEATING container prefix a blockquote's
//! own marker scan already (and correctly) claims.
//!
//! - Setext heading nested in a container (verification round 4 MAJOR,
//!   second site): `HeadingM::range` spans BOTH the text line and the
//!   "==="/"---" underline — fixed the same way `CodeFenceM` already was,
//!   per-line `content_lines` built at parse time.
//! - Thematic break before an empty blockquote continuation line
//!   (verification round 5 CLASS B): comrak's reported sourcepos for a
//!   thematic break immediately followed by an EMPTY blockquote
//!   continuation line extended through that next line's own "> " marker
//!   — fixed by clamping `HrM::range` to its own single line.
//! - Blockquote marker mid-buffer-line after a lone `\r` (verification
//!   round 7, EMIT-ORDER fallout): making blockquote markers comrak-line-
//!   aware means a marker can legitimately sit MID-buffer-line — fixed by
//!   sorting each line's spans by `buffer_start` once at the `emit()`
//!   chokepoint.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod conceal_common;

use conceal_common::{assert_full_line_coverage, assert_no_duplicate_content, joined_line, synced};
use rune_md::emit::emit;

#[test]
fn setext_heading_nested_in_double_blockquote_does_not_double_claim() {
    assert_no_duplicate_content("> > nested\n> > ---");
}

#[test]
fn setext_heading_nested_in_blockquote_does_not_double_claim() {
    assert_no_duplicate_content("> nested\n> ---");
}

#[test]
fn setext_heading_nested_in_list_item_does_not_double_claim() {
    assert_no_duplicate_content("- nested\n  ---");
}

#[test]
fn setext_heading_with_trailing_content_lines_stays_clean() {
    // Content AFTER the underline (a later continuation line, nested or
    // not) must stay unaffected — the fix only changes how the heading's
    // OWN two lines are split, never anything past them.
    assert_no_duplicate_content("> nested\n> ---\n> more text");
    assert_no_duplicate_content("nested\n---\nafter");
}

#[test]
fn thematic_break_before_empty_quote_continuation_does_not_double_claim() {
    assert_no_duplicate_content("> ---\n>");
    assert_no_duplicate_content("> ---\n> ");
    assert_no_duplicate_content("> ---\n>\n");
    assert_no_duplicate_content("> ---\n> \n");
}

#[test]
fn thematic_break_empty_continuation_controls_stay_clean() {
    // The reviewer's clean controls: a NON-empty continuation line, "==="
    // (not a valid thematic break marker, so a different node kind
    // entirely), a setext heading (a REAL multi-line construct) followed
    // by an empty continuation, and a doubly-nested empty continuation.
    assert_no_duplicate_content("> ---\n> x");
    assert_no_duplicate_content(">===\n>");
    assert_no_duplicate_content("> a\n> ---\n>");
    assert_no_duplicate_content("> ---\n>>");
}

#[test]
fn blockquote_marker_mid_buffer_line_after_lone_cr_stays_in_order() {
    let content = "[[\n]]\na\r> q\na\r> q";
    assert_no_duplicate_content(content);
    let (buf, doc) = synced(content, content.len(), true);
    let (lines, snap) = emit(buf.content(), doc.blocks(), 80);
    assert_full_line_coverage(&buf, &lines, &snap);
    for line in 0..buf.line_count() {
        if snap.hidden_byte_count(line) == 0 {
            assert_eq!(
                joined_line(&lines, line, buf.content()),
                buf.line(line),
                "line {line}: rendered text out of byte order"
            );
        }
    }
}
