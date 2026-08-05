//! Split off `conceal_roundtrip.rs` (WP11): fence-inside-container
//! regression cases (verification-round BLOCKER). `fence_open`/`content`/
//! `fence_close` used to be derived from PHYSICAL line extents
//! (`line_start_at`/`line_end_at`), ignoring the enclosing container's own
//! prefix already claimed on that line — the fence's ranges swallowed bytes
//! the container's marker had already hidden (or, for the Revealed dump,
//! already shown), so every position past the collision was off by the
//! doubly-counted delta. Checked in BOTH focus states, per line: full byte
//! coverage, `buffer_to_syntax` monotonic non-decreasing across the line,
//! and round-trip stability.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod conceal_common;

use conceal_common::{assert_full_line_coverage, synced};
use rune_core::coords::BufferPoint;
use rune_md::emit::emit;

fn assert_container_fence_invariants(content: &str) {
    for &focused in &[true, false] {
        let (buf, doc) = synced(content, 0, focused);
        let (lines, snap) = emit(buf.content(), doc.blocks(), 80);
        assert_full_line_coverage(&buf, &lines, &snap);

        for line in 0..buf.line_count() {
            let line_len = buf.line(line).len();
            let mut prev_syntax_col = None;
            for col in 0..=line_len {
                let bp = BufferPoint { line, col };
                let sp = snap.buffer_to_syntax(bp);
                if let Some(prev) = prev_syntax_col {
                    assert!(
                        sp.col >= prev,
                        "buffer_to_syntax not monotonic (focused={focused}) at line {line} col {col}: prev={prev} now={}",
                        sp.col
                    );
                }
                prev_syntax_col = Some(sp.col);

                // Round-trip stability: buffer_to_syntax(syntax_to_buffer(sp))
                // == sp for every syntax point reachable from this line.
                let bp2 = snap.syntax_to_buffer(sp);
                let sp2 = snap.buffer_to_syntax(bp2);
                assert_eq!(
                    sp, sp2,
                    "round-trip stability failed (focused={focused}) at line {line} col {col}: bp={bp:?} sp={sp:?} bp2={bp2:?} sp2={sp2:?}"
                );
            }
        }
    }
}

#[test]
fn fence_inside_blockquote_container_prefix_not_double_hidden() {
    // The reviewer's exact repro: unfocused line 0 used to report
    // visible=0 + hidden=11 on a 9-byte line ("> ```rust"), and
    // syntax_to_buffer(0) returned col 11 — out of the line entirely.
    assert_container_fence_invariants("> ```rust\n> fn main() {}\n> ```\n");
}

#[test]
fn fence_inside_bare_blockquote_container_prefix_not_double_hidden() {
    assert_container_fence_invariants("> ```\n> code\n> ```\n");
}

#[test]
fn fence_inside_nested_blockquote_container_prefix_not_double_hidden() {
    assert_container_fence_invariants("> > ```\n> > c\n> > ```\n");
}

#[test]
fn fence_inside_list_item_container_prefix_not_double_hidden() {
    // fence_open on line 0 used to span the whole physical line
    // ("- ```rust"), colliding with the list item's own marker [0,2).
    assert_container_fence_invariants("- ```rust\n  code\n  ```\n");
}

#[test]
fn fence_with_multiple_content_lines_inside_blockquote() {
    // Every content line (not just the first/last) carries the
    // container's repeating "> " prefix — this is the shape a single
    // contiguous `content` range could never handle correctly.
    assert_container_fence_invariants("> ```rust\n> line1\n> line2\n> ```\n");
}
