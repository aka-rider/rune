//! Split off `conceal_roundtrip.rs` (WP11): wikilink label-range
//! regression cases (verification round 3 MAJOR): the label range used to
//! be read off comrak's own child-node sourcepos, which is unreliable for a
//! WikiLink target with leading whitespace that gets trimmed — off by one
//! for ASCII ("[[ a]]" showed " " instead of " a"), and char-splitting/
//! out-of-range for a multibyte final char ("[[ 日]]", "[[ 👍]]"), which
//! used to hit the emit-site `else { continue }` and silently drop the
//! whole span (bytes unaccounted for — the round-1 byte-loss class at a new
//! site). Checked revealed (cursor on the wikilink's own line), concealed
//! (cursor on an unrelated line), and unfocused (always concealed) — plus
//! full per-line byte coverage in every state.
//!
//! Also carries the residual-producer and emphasis/strikethrough-wrapping
//! regression cases from the same wikilink desync family (verification
//! rounds 3-4), plus the lone-`\r`-as-line-break wikilink case (round 5).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod conceal_common;

use conceal_common::{joined_line, synced};
use rune_md::emit::emit;
use rune_md::invariant::{assert_full_line_coverage, assert_no_duplicate_content};

fn assert_wikilink_label(content: &str, concealed_label: &str) {
    // Revealed: cursor ON the wikilink's own line shows the raw markup
    // verbatim — nothing about revealing depends on the label arithmetic.
    let (buf, doc) = synced(content, 0, true);
    let (lines, snap) = emit(buf.content(), doc.blocks(), 80);
    assert_full_line_coverage(&buf, &lines, &snap);
    assert_eq!(
        joined_line(&lines, 0, buf.content()),
        content.trim_end_matches('\n')
    );

    // Concealed: cursor on an unrelated line shows just the label.
    let wrapped = format!("x\n{content}");
    let (buf, doc) = synced(&wrapped, 0, true);
    let (lines, snap) = emit(buf.content(), doc.blocks(), 80);
    assert_full_line_coverage(&buf, &lines, &snap);
    assert_eq!(joined_line(&lines, 1, buf.content()), concealed_label);

    // Unfocused: always concealed regardless of cursor position.
    let (buf, doc) = synced(content, 0, false);
    let (lines, snap) = emit(buf.content(), doc.blocks(), 80);
    assert_full_line_coverage(&buf, &lines, &snap);
    assert_eq!(joined_line(&lines, 0, buf.content()), concealed_label);
}

#[test]
fn wikilink_label_with_leading_space_ascii_is_byte_exact() {
    assert_wikilink_label("[[ a]]\n", " a");
}

#[test]
fn wikilink_label_with_leading_space_cjk_is_byte_exact() {
    assert_wikilink_label("[[ 日]]\n", " 日");
}

#[test]
fn wikilink_label_with_leading_space_emoji_is_byte_exact() {
    assert_wikilink_label("[[ 👍]]\n", " 👍");
}

// ---------------------------------------------------------------------
// Residual-producer regression cases (verification round 3: advisory
// promoted to work): two inputs still tripped the strict-invariants
// assert, saved only by the emit-site chokepoint — both producers now
// line-clamp/disjoint their claims like every other producer in this
// crate, so these are green even under strict mode.
// ---------------------------------------------------------------------

#[test]
fn multiline_wikilink_does_not_claim_across_lines() {
    // A wikilink whose own sourcepos spans more than one physical line
    // degrades to plain text — no single-line home for open/close
    // delimiter claims exists, so it must never reach WikiLinkM
    // construction in the first place.
    for &focused in &[true, false] {
        let (buf, doc) = synced("[[\n]]\n", 0, focused);
        let (lines, snap) = emit(buf.content(), doc.blocks(), 80);
        assert_full_line_coverage(&buf, &lines, &snap);
    }
}

#[test]
fn tab_indented_blockquote_continuation_does_not_double_claim() {
    // comrak treats a TAB-indented continuation line as lazy-continuation
    // PARAGRAPH TEXT (CommonMark: a repeated container marker may only be
    // preceded by 0-3 SPACES, never a tab, which represents 4 columns) —
    // `blockquote_markers` used to recognize it as a repeated ">" marker
    // anyway (`str::trim_start` strips tabs too), double-claiming the
    // same byte the paragraph's own Text node also claims: a
    // producer-bug duplicate-claim panic under strict invariants.
    assert_no_duplicate_content(">]\n\t>");
}

// ---------------------------------------------------------------------
// Emphasis/strikethrough wrapping a multi-line wikilink (verification
// round 4 MAJOR): comrak's line-counter desync (round 3's "[[\n]]" root
// cause) doesn't stop at the wikilink's own siblings — a PARENT wrapping
// it is exposed too, because its own `child_gap_delims` reads the LAST
// child's (possibly corrupted) sourcepos to place the close delimiter.
// `"*[[\n]]\n(*"`: the closing "*" got recorded hidden on the wikilink's
// own line while the emitter placed it, unhidden, on the real closing
// line — a coverage/duplicate-claim bug at a new site, same root cause as
// round 3's residual producers.
// ---------------------------------------------------------------------

#[test]
fn emphasis_wrapped_multiline_wikilink_does_not_double_claim() {
    assert_no_duplicate_content("*[[\n]]\n(*");
    assert_no_duplicate_content("*[[\n]]\n-*");
}

#[test]
fn strikethrough_wrapped_multiline_wikilink_does_not_double_claim() {
    assert_no_duplicate_content("~~[[\n]]\nb~~");
}

#[test]
fn multiline_wikilink_without_wrapper_stays_clean() {
    // The reviewer's clean control: without the Emphasis/Strikethrough
    // wrapper, a bare multi-line wikilink followed by more content was
    // already correctly handled by round 3's fix — pinned here so a
    // future change can't silently regress the unwrapped case while
    // fixing the wrapped one.
    assert_no_duplicate_content("[[\n]]\n(");
}

#[test]
fn wikilink_matching_a_lone_cr_is_treated_as_a_line_break() {
    // `subtree_has_multiline_wikilink` decides whether a `[[...]]` match
    // could have desynced comrak's own internal line counter (verification
    // rounds 3-4) by checking whether the match's own text contains a
    // raw newline — but it only checked `'\n'`. `idx.comrak` (and
    // comrak's own line counter, the thing this check exists to predict)
    // treats a LONE `\r` as a line terminator exactly like `\n` (CR/LF/
    // CRLF all count). A wikilink matching a bare `\r` (no following
    // `\n`) desyncs comrak's line counter the same way an embedded `\n`
    // does, but went undetected, leaving a corrupted sibling's range to
    // collide with an already-claimed byte.
    assert_no_duplicate_content("[[\r]]\n[");
    assert_no_duplicate_content("[[\r]]\nx");
    assert_no_duplicate_content("x [[a\rb]] **bold**\r");
}
