//! Tests for `syntax`'s core machinery, split out to keep the owning
//! module under the 500-line budget.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use super::*;

/// `CellMap` is per-CHAR (one entry per `char`, `chars().count()`
/// entries) despite the name — easy to conflate with a terminal display
/// cell. Pins the length against BOTH a combining-mark cluster (two
/// `char`s, one grapheme cluster) and a double-width CJK char (one
/// `char`, two display cells): `substituted` produces one entry per
/// `char` in every case, never one per grapheme cluster and never one
/// per display cell.
#[test]
fn substituted_cell_map_has_one_entry_per_char_not_per_grapheme_or_display_cell() {
    // "é" as `e` + COMBINING ACUTE ACCENT: 2 `char`s, ONE grapheme cluster.
    let combining = "e\u{0301}";
    let span = SyntaxSpan::substituted(10, combining.to_string(), ScopeId(0), 10..13);
    let SyntaxSpan::Substituted { cell_map, .. } = &span else {
        panic!("expected Substituted");
    };
    assert_eq!(cell_map.len(), combining.chars().count());
    assert_eq!(cell_map, &vec![Some(10), Some(11)]); // 'e' is 1 byte; the combining mark starts at 11

    // "世界": each char is ONE 3-byte codepoint but TWO display cells —
    // the map still has exactly one entry per char, at each char's own
    // byte start, never per display cell.
    let cjk = "世界";
    let span = SyntaxSpan::substituted(0, cjk.to_string(), ScopeId(0), 0..6);
    let SyntaxSpan::Substituted { cell_map, .. } = &span else {
        panic!("expected Substituted");
    };
    assert_eq!(cell_map.len(), cjk.chars().count());
    assert_eq!(cell_map, &vec![Some(0), Some(3)]);
}

/// The structural-hardening chokepoint: overlapping input intervals
/// merge into the minimal disjoint set covering the SAME bytes exactly
/// once — an overlapping-range producer bug degrades to a coordinate
/// inaccuracy (caught separately by the strict-invariants-gated
/// assert below, in tests), never a doubly-counted delta.
#[test]
fn merge_overlapping_intervals_counts_each_byte_once() {
    // [0,5) and [3,8) share bytes [3,5) — merged into one [0,8): 8
    // bytes total, NOT 5+5=10 (which is what a naive unmerged sum
    // would produce, the exact "delta summed twice" shape reported for
    // a fence's ranges colliding with its container's marker ranges).
    let merged = merge_overlapping(vec![(0, 5), (3, 8), (10, 12)]);
    assert_eq!(merged, vec![(0, 8), (10, 12)]);
    let total_bytes: usize = merged.iter().map(|&(s, e)| e - s).sum();
    assert_eq!(total_bytes, 10); // 8 (merged) + 2, not 5+5+2=12

    // Touching-but-not-overlapping ranges also merge (no shared byte,
    // but no gap either): [0,4) and [4,9).
    let touching = merge_overlapping(vec![(0, 4), (4, 9)]);
    assert_eq!(touching, vec![(0, 9)]);
}

#[test]
fn has_overlap_distinguishes_overlap_from_mere_adjacency() {
    assert!(has_overlap(&[(0, 5), (3, 8)])); // shares bytes [3,5)
    assert!(!has_overlap(&[(0, 4), (4, 9)])); // touches at 4, no shared byte
    assert!(!has_overlap(&[(0, 2), (5, 7)])); // disjoint with a gap
    assert!(!has_overlap(&[]));
}

/// Proves the strict-invariants-gated assert in
/// `build_line_conversions` is actually wired to fire on overlapping
/// input — the two prior findings on this branch (a fence's ranges
/// colliding with its container's marker ranges) were both this exact
/// shape, and would have tripped this assertion in tests immediately
/// instead of silently corrupting coordinate conversion. Unlike the
/// old `debug_assert!`-based version, the strict-invariants gate is
/// tied to `cfg(test)` (not `cfg(debug_assertions)`), so this fires in
/// a `--release` test run too (the assert is test-only, not
/// profile-only — a `cargo test --release` run must still catch this).
#[test]
#[should_panic(expected = "overlapping hidden ranges")]
fn build_line_conversions_debug_asserts_on_overlapping_input() {
    // Two overlapping ranges on line 0: [0,5) and [3,8).
    let starts = vec![0usize];
    let hidden = vec![vec![(0usize, 5usize), (3usize, 8usize)]];
    let _ = build_line_conversions(&starts, &hidden);
}

/// A zero-length raw range (`e == s`) must be dropped before the
/// overlap check ever sees it — not merely at merge time. Here the
/// zero-length entry `(5,5)` sorts ahead of the real `(5,8)` range once
/// both survive the filter, which would make the (wrongly kept)
/// zero-length entry look like it overlaps its neighbor and trip the
/// producer-bug assert on perfectly valid input.
#[test]
fn build_line_conversions_drops_zero_length_ranges_before_the_overlap_check() {
    let starts = vec![0usize];
    let hidden = vec![vec![(5usize, 8usize), (5usize, 5usize)]];
    let convs = build_line_conversions(&starts, &hidden);
    assert_eq!(convs[0].hidden.len(), 1);
    assert_eq!(convs[0].hidden[0].start, 5);
    assert_eq!(convs[0].hidden[0].end, 8);
    assert_eq!(convs[0].deltas[0].delta, 3);
}

/// `clamp_col` treats a hidden range as `[start, end)`: the byte right
/// after a hidden run (`col == h.end`) is a normal, unclamped syntax
/// column, not part of the range it just walked past.
#[test]
fn clamp_col_end_is_exclusive() {
    let hidden = vec![HiddenRange {
        start: 5,
        end: 8,
        clamp_to: 99,
    }];
    assert_eq!(clamp_col(8, &hidden), 8);
    assert_eq!(clamp_col(7, &hidden), 99);
}

/// `clamp_col`'s early-exit condition (`h.start > col`, "no earlier
/// range could possibly match, so none of the rest can either") must
/// fire on strictly-greater and nothing weaker. A range whose start
/// merely EQUALS `col` never matches on its own (a hidden range is
/// never empty in practice) but must NOT trigger the early exit either,
/// since a later, real range starting at that same `col` still can.
#[test]
fn clamp_col_stops_scanning_only_once_past_every_possible_match() {
    let starts_after_col = vec![
        HiddenRange {
            start: 20,
            end: 25,
            clamp_to: 25,
        },
        HiddenRange {
            start: 3,
            end: 6,
            clamp_to: 6,
        },
    ];
    assert_eq!(clamp_col(4, &starts_after_col), 4);

    let degenerate_then_real = vec![
        HiddenRange {
            start: 5,
            end: 5,
            clamp_to: 999,
        },
        HiddenRange {
            start: 5,
            end: 8,
            clamp_to: 77,
        },
    ];
    assert_eq!(clamp_col(5, &degenerate_then_real), 77);

    let earlier_non_match_then_real = vec![
        HiddenRange {
            start: 2,
            end: 5,
            clamp_to: 5,
        },
        HiddenRange {
            start: 8,
            end: 12,
            clamp_to: 12,
        },
    ];
    assert_eq!(clamp_col(9, &earlier_non_match_then_real), 12);
}
