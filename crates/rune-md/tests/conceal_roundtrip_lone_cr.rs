//! Split off `conceal_roundtrip.rs` (WP11): CLASS A (verification
//! round 5) — a lone `\r` line terminator. comrak follows CommonMark: CR,
//! LF, or CRLF all end a line. This crate's BUFFER line model is `\n`-only
//! — correctly so, a bare `\r` is ordinary mid-line content, never a
//! buffer line break. But `sourcepos_to_range` used to convert comrak's
//! own (CR/LF/CRLF-aware) sourcepos through that SAME `\n`-only index, so
//! the moment content contained a bare `\r`, comrak's line N stopped
//! matching this crate's line N and every downstream byte offset landed
//! on the wrong physical position. Fixed with a SECOND, comrak-compatible
//! line index (`LineIndex::comrak`) used ONLY for sourcepos conversion —
//! the bytes themselves are never touched.
//!
//! Also carries the MIXED-INDEX SEAM cases (verification round 7 BLOCKER):
//! a fence's own internal physical-line arithmetic still used the buffer's
//! `\n`-only index even after round 5 taught sourcepos conversion to use
//! the comrak-compatible one — for a document with no `\n` at all, the
//! buffer's `\n`-only line count collapsed the whole fence onto one
//! physical line, silently dropping everything past the fence's own start.
//! Fixed by deriving all of this per-line arithmetic from `idx.comrak`.
//!
//! Also carries a case where two sibling text runs straddling a lone `\r`
//! inside an unclosed backtick sequence, nested under a list item and a
//! blockquote, derived overlapping per-line ranges for the same buffer
//! line.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod conceal_common;

use conceal_common::{joined_line, synced};
use rune_md::emit::emit;
use rune_md::invariant::assert_no_duplicate_content;

#[test]
fn lone_cr_before_list_marker_does_not_desync() {
    assert_no_duplicate_content("a\r- n");
}

#[test]
fn lone_cr_before_fence_does_not_desync() {
    assert_no_duplicate_content("a\r```");
}

#[test]
fn lone_cr_before_heading_does_not_desync() {
    assert_no_duplicate_content("a\r# h");
}

#[test]
fn lone_cr_controls_stay_clean() {
    // The reviewer's clean controls: plain text and a blockquote after a
    // lone CR, plus a CR with nothing following it at all.
    assert_no_duplicate_content("a\rb");
    assert_no_duplicate_content("a\r> q");
    assert_no_duplicate_content("a\r");
}

#[test]
fn crlf_before_list_marker_stays_clean() {
    // The CRLF control: CRLF is ONE terminator, not two — must NOT be
    // treated as a lone CR immediately followed by a lone LF (which
    // would double-count a line break that only happened once).
    assert_no_duplicate_content("a\r\n- n");
}

#[test]
fn lone_cr_inside_frontmatter_does_not_desync_the_rest_of_the_document() {
    // A THIRD comrak-extension desync, found by this round's own widened
    // generator (not part of the reviewer's original CLASS A report): a
    // lone `\r` inside a frontmatter block's body throws off comrak's
    // frontmatter-closing search (which appears to scan by `\n`-only
    // splitting internally) relative to the CR/LF/CRLF-aware line
    // counter the REST of comrak's block parser keeps counting from
    // afterward — corrupting every LATER block's sourcepos too, not just
    // the frontmatter block's own (the wikilink-extension desync's
    // document-wide sibling). `parse()`'s `frontmatter_extension_is_safe`
    // pre-check detects this and re-parses with the extension disabled.
    assert_no_duplicate_content("---\na\r```\n---\n> nested");
}

#[test]
fn lone_cr_fence_does_not_swallow_the_rest_of_the_document() {
    let content = "a\r```\rc\r```";
    assert_no_duplicate_content(content);
    for &focused in &[true, false] {
        let (buf, doc) = synced(content, 0, focused);
        let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
        let joined = joined_line(&lines, 0, buf.content());
        assert!(
            joined.contains('c'),
            "fence content 'c' missing from rendered output (focused={focused}): {joined:?}"
        );
    }
}

#[test]
fn classic_mac_readme_shape_does_not_lose_fence_or_quote_content() {
    // A realistic multi-construct classic-Mac-line-ending document:
    // heading, intro paragraph, a fenced code block, and a blockquote —
    // exactly the shape verification round 7 found losing content in a
    // real README render ("TIntro", code and quote text both gone).
    let content = "# T\rIntro\r\r```\rcode\r```\r\r> quote\r";
    assert_no_duplicate_content(content);
    for &focused in &[true, false] {
        let (buf, doc) = synced(content, 0, focused);
        let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
        let joined = joined_line(&lines, 0, buf.content());
        assert!(
            joined.contains("code"),
            "fence content 'code' missing from rendered output (focused={focused}): {joined:?}"
        );
        assert!(
            joined.contains("quote"),
            "blockquote content 'quote' missing from rendered output (focused={focused}): {joined:?}"
        );
    }
}

#[test]
fn lone_cr_fence_controls_stay_clean() {
    assert_no_duplicate_content("# T\rp");
    assert_no_duplicate_content("# T\r- a");
    assert_no_duplicate_content("a\r> q");
    assert_no_duplicate_content("# T\rp\r\r- one\r- two");
}

#[test]
fn lone_cr_backtick_runs_inside_nested_quote_do_not_double_claim() {
    assert_no_duplicate_content("-\n  > nested\n  > more\na\r```\na\r```\nplain text");
}

#[test]
fn lone_cr_fence_lf_equivalents_stay_clean() {
    assert_no_duplicate_content("a\n```\nc\n```");
    assert_no_duplicate_content("# T\np");
    assert_no_duplicate_content("# T\n- a");
    assert_no_duplicate_content("a\n> q");
    assert_no_duplicate_content("# T\np\n\n- one\n- two");
}
