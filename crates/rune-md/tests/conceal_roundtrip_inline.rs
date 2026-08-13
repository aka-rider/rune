//! Split off `conceal_roundtrip.rs` (WP11): MAJOR (verification
//! round 9, second and third fuzz-driven pass) — the same exhaustive audit
//! that found `Block::Verbatim` missing container-aware per-line treatment
//! found the identical gap in inline constructs: `EmphasisM`'s Revealed-
//! path `range` has the same un-clamped-multi-line-range shape, and
//! `InlineCodeM` has TWO separate multi-line-capable fields — the
//! Revealed-path `range` AND the Rendered-path `content` (the code span's
//! own inner text). All now route through the shared `per_line_content`
//! chokepoint.
//!
//! `inline_code_close_delimiter_is_located_not_computed_arithmetically`
//! pins two independent close-delimiter bugs: naive `range.end` arithmetic
//! no longer locates the real closing backticks once per-line marker
//! widths can differ, or once comrak's own outer sourcepos extends past
//! the true close run — fixed by LOCATING the close run instead of
//! computing its position by subtraction.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod conceal_common;

use conceal_common::{joined_line, synced};
use rune_md::element::block::Block;
use rune_md::element::inline::{Inline, InlineCodeM};
use rune_md::emit::emit;
use rune_md::invariant::assert_no_duplicate_content;
use rune_syntax::element::ByteRange;

#[test]
fn multiline_emphasis_strong_strikethrough_in_blockquote_stays_in_order() {
    // Found by this round's own exhaustive audit (not in the original
    // ticket): `EmphasisM`'s Revealed-path `range` has the SAME
    // un-clamped-multi-line-range shape `VerbatimM` did.
    for content in [
        "> *a\n> b*",
        "> **a\n> b**",
        "> ~~a\n> b~~",
        "> *a\n> b*\n> more",
        "> > *a\n> > b*",
    ] {
        assert_no_duplicate_content(content);
    }
    let content = "> *a\n> b*";
    // Cursor INSIDE the emphasis span (on "a") reveals the whole multi-line
    // emphasis token as a unit — but the blockquote marker itself reveals
    // per LINE independently (see `blockquote_marker_reveals_per_line_
    // independently` above), so only line 0's own "> " shows.
    let cursor = content.find('a').expect("fixture contains 'a'");
    let (buf, doc) = synced(content, cursor, true);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    assert_eq!(joined_line(&lines, 0, buf.content()), "> *a");
    assert_eq!(joined_line(&lines, 1, buf.content()), "b*");
    // Cursor OUTSIDE the emphasis span (line 0 only) conceals the
    // delimiters and, per-line, line 1's own blockquote marker too.
    let (buf, doc) = synced(content, 0, true);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    assert_eq!(joined_line(&lines, 0, buf.content()), "> a");
    assert_eq!(joined_line(&lines, 1, buf.content()), "b");
}

#[test]
fn multiline_inline_code_in_blockquote_shows_content_both_states() {
    // Found by the same audit: `InlineCodeM` has TWO separate multi-line-
    // capable fields — the Revealed-path `range` (same shape as
    // Emphasis) AND the Rendered-path `content` (the code span's own
    // inner text, concealed state) — both used to re-claim the
    // continuation line's own "> " marker.
    let content = "> `a\n> b`\n> more";
    assert_no_duplicate_content(content);
    for &focused in &[true, false] {
        let (buf, doc) = synced(content, 0, focused);
        let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
        // Concealed (cursor away from the code span): rendered code text
        // must show "a" and "b" without duplicating the quote marker.
        assert!(joined_line(&lines, 0, buf.content()).contains('a'));
        assert!(joined_line(&lines, 1, buf.content()).contains('b'));
    }
    // Revealed (cursor INSIDE the code span, on "a"): raw backticks show;
    // the blockquote marker still reveals per line independently, so only
    // line 0 (the cursor's own line) keeps its "> ".
    let cursor = content.find('a').expect("fixture contains 'a'");
    let (buf, doc) = synced(content, cursor, true);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    assert_eq!(joined_line(&lines, 0, buf.content()), "> `a");
    assert_eq!(joined_line(&lines, 1, buf.content()), "b`");
}

#[test]
fn multiline_link_text_in_blockquote_stays_in_order() {
    let content = "> [a\n> b](url)\n> more";
    assert_no_duplicate_content(content);
    // Revealed (cursor INSIDE the link's text span, on "a"); the blockquote
    // marker reveals per line independently, so only line 0 keeps "> ".
    let cursor = content.find('a').expect("fixture contains 'a'");
    let (buf, doc) = synced(content, cursor, true);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    assert_eq!(joined_line(&lines, 0, buf.content()), "> [a");
    assert_eq!(joined_line(&lines, 1, buf.content()), "b](url)");
    // Concealed (cursor away from the link, line 0 only): text shows,
    // markup hidden, and line 1's own blockquote marker conceals too.
    let (buf, doc) = synced(content, 0, true);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    assert_eq!(joined_line(&lines, 0, buf.content()), "> a");
    assert_eq!(joined_line(&lines, 1, buf.content()), "b");
}

#[test]
fn multiline_inline_variants_in_list_item_stay_clean() {
    // List items have no REPEATING marker, so these were never actually
    // reachable — pinned as controls confirming that stays true.
    assert_no_duplicate_content("- *a\n  b*");
    assert_no_duplicate_content("- `a\n  b`");
    assert_no_duplicate_content("- [a\n  b](url)");
}

#[test]
fn inline_code_close_delimiter_is_located_not_computed_arithmetically() {
    // `InlineCodeM::open`/`close` used to be derived by subtracting
    // `num_backticks` straight off the outer `range`'s start/end — safe
    // ONLY when `range` is contiguous raw bytes. Two independent bugs
    // broke that assumption: (1) a multi-line code span crossing a
    // blockquote's lazy-continuation line (no "> " at all) followed by a
    // bare "> " (no trailing space, narrower than usual) has mismatched
    // per-line marker widths, so naive arithmetic on `range.end` no
    // longer lands on the real closing backticks; (2) independently of
    // any container, comrak's OWN sourcepos for a `Code` node's outer
    // span can extend one or more bytes past the true close run (e.g. a
    // trailing space folded in after the CommonMark line-ending-to-space
    // conversion). Both are fixed by LOCATING the close run (scan
    // backward over non-backtick bytes, then take the trailing run)
    // instead of computing its position by subtraction.
    assert_no_duplicate_content("]\n x```\n``` `");
    assert_no_duplicate_content(">t\n>`\n`>");
    // A realistic shape of bug (1): a code span opened on a blockquote's
    // lazy-continuation line, closed on a line with a bare ">" (no
    // trailing space).
    assert_no_duplicate_content("> a\n`b\n>`c");
    // The narrowest shape of bug (1): a lazy-continuation line carrying
    // NOTHING but a single unmatched backtick, followed by an empty bare
    // quote line.
    assert_no_duplicate_content(">c\n`\n>");
}

#[test]
fn multiline_code_span_extent_stops_at_its_closing_backtick() {
    let content = "a\n `\n`x";
    let (_buf, doc) = synced(content, 0, true);
    let code = first_code_span(doc.blocks()).expect("fixture contains a code span");
    assert_eq!(code.open(), ByteRange::new(3, 4));
    assert_eq!(code.close(), ByteRange::new(5, 6));
    assert_eq!(code.range(), ByteRange::new(3, 6));
    assert_eq!(code.content(), ByteRange::new(4, 5));
}

fn first_code_span(blocks: &[Block]) -> Option<&InlineCodeM> {
    blocks.iter().find_map(|block| match block {
        Block::Paragraph(p) => p.inlines.iter().find_map(|inline| match inline {
            Inline::Code(m) => Some(m),
            _ => None,
        }),
        _ => None,
    })
}
