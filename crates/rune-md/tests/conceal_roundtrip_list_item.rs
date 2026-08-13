//! Split off `conceal_roundtrip.rs` (WP11): empty-list-item-marker
//! regression cases (verification round 3 BLOCKER). An empty item's marker
//! ran from the item's own start to its FIRST CHILD's start — which, for a
//! lazily-indented continuation (e.g. a nested blockquote under
//! "- \n  > q"), sits on the NEXT physical line. The marker swallowed that
//! line's leading indent, bytes the continuation's own scan
//! (`blockquote_markers`) claims independently: content invented on the
//! visible side (both spans show the same 2 bytes) — the mirror image of
//! dropping a byte: either way, the bytes don't round-trip verbatim.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod conceal_common;

use rune_md::invariant::assert_no_duplicate_content;

#[test]
fn empty_list_item_marker_does_not_duplicate_continuation_indent() {
    // The reviewer's exact repro: buffer line 1 is "  > q" (5 bytes) but
    // the marker used to swallow the 2-space indent a second time,
    // emitting "    > q" (7 bytes).
    assert_no_duplicate_content("- \n  > q");
}

#[test]
fn empty_item_variants_times_continuation_matrix() {
    let empty_markers = ["-", "- ", "*", "+", "1.", "\t-"];
    let continuations = ["  > q", "> q", "  x", "x"];
    for m in empty_markers {
        for c in continuations {
            assert_no_duplicate_content(&format!("{m}\n{c}"));
        }
    }
}

#[test]
fn empty_item_continuation_controls_stay_clean() {
    // Known-good controls that must remain clean.
    assert_no_duplicate_content("- a\n  > q");
    assert_no_duplicate_content("-\n>");
    assert_no_duplicate_content("-\n  x");
}

#[test]
fn heading_leading_a_list_item_does_not_shift_byte_accounting() {
    // The heading-wins decor rule (suppressing the bullet's `LineDecor`
    // piece) must not touch a single hidden/visible byte — it only changes
    // which decor pieces get pushed, never `hide_range`/`claim_visible`.
    assert_no_duplicate_content("- # h");
    assert_no_duplicate_content("- ## h");
    assert_no_duplicate_content("1. # h");
    assert_no_duplicate_content("- text\n\n  # h");
    assert_no_duplicate_content("- [ ] x");
    assert_no_duplicate_content("- a\n  - b");
}
