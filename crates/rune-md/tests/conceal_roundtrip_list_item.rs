//! Split off `conceal_roundtrip.rs` (WP11, §1.6): empty-list-item-marker
//! regression cases (verification round 3 BLOCKER). An empty item's marker
//! ran from the item's own start to its FIRST CHILD's start — which, for a
//! lazily-indented continuation (e.g. a nested blockquote under
//! "- \n  > q"), sits on the NEXT physical line. The marker swallowed that
//! line's leading indent, bytes the continuation's own scan
//! (`blockquote_markers`) claims independently: content invented on the
//! visible side (both spans show the same 2 bytes) — §1.4.5's mirror image
//! of dropping a byte.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod conceal_common;

use conceal_common::assert_no_duplicate_content;

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
