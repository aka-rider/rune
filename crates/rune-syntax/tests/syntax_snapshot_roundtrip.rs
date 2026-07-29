//! [rune-syntax 6]: `buffer_to_syntax`/`syntax_to_buffer` round-trips across
//! hidden ranges — the crate's own coordinate-conversion contract,
//! previously exercised only from `rune-md`/`rune-fuzz`, never in-crate.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use rune_core::coords::{BufferPoint, SyntaxPoint};
use rune_syntax::SyntaxSnapshot;

/// One line, one hidden range `[2, 5)` (e.g. a 3-byte concealed delimiter
/// like a fence's own backticks) starting at buffer offset 0.
fn one_hidden_range() -> SyntaxSnapshot {
    SyntaxSnapshot::build(&[0], &[vec![(2, 5)]])
}

#[test]
fn a_point_before_the_hidden_range_is_unaffected() {
    let snap = one_hidden_range();
    let sp = snap.buffer_to_syntax(BufferPoint { line: 0, col: 1 });
    assert_eq!(sp, SyntaxPoint { line: 0, col: 1 });
    let bp = snap.syntax_to_buffer(sp);
    assert_eq!(bp, BufferPoint { line: 0, col: 1 });
}

#[test]
fn a_point_after_the_hidden_range_shifts_left_by_its_length() {
    let snap = one_hidden_range();
    // Buffer col 8 is 3 bytes past the hidden range's end (5); syntax space
    // has those 3 bytes removed, so it lands at col 5.
    let sp = snap.buffer_to_syntax(BufferPoint { line: 0, col: 8 });
    assert_eq!(sp, SyntaxPoint { line: 0, col: 5 });
    let bp = snap.syntax_to_buffer(sp);
    assert_eq!(bp, BufferPoint { line: 0, col: 8 });
}

#[test]
fn a_point_inside_the_hidden_range_clamps_to_its_start_in_syntax_space() {
    let snap = one_hidden_range();
    // Buffer col 3 and col 4 both sit inside the hidden [2,5) range; both
    // clamp to the same syntax-space position (the range's own start, 2,
    // with no bytes hidden before it yet).
    let sp3 = snap.buffer_to_syntax(BufferPoint { line: 0, col: 3 });
    let sp4 = snap.buffer_to_syntax(BufferPoint { line: 0, col: 4 });
    assert_eq!(sp3, SyntaxPoint { line: 0, col: 2 });
    assert_eq!(
        sp3, sp4,
        "every position inside a hidden range clamps identically"
    );
}

#[test]
fn round_trip_holds_for_every_buffer_position_outside_the_hidden_range() {
    let snap = one_hidden_range();
    // Buffer bytes [0,2) and [5,12) are visible; every one of those
    // round-trips exactly (the reverse direction is only guaranteed for
    // visible positions — a hidden-range position is lossy by definition,
    // covered by the clamp test above).
    for col in (0..2).chain(5..12) {
        let sp = snap.buffer_to_syntax(BufferPoint { line: 0, col });
        let bp = snap.syntax_to_buffer(sp);
        assert_eq!(
            bp,
            BufferPoint { line: 0, col },
            "buffer col {col} did not round-trip through syntax space"
        );
    }
}

#[test]
fn two_hidden_ranges_on_the_same_line_both_shift_the_delta() {
    // [2,5) then [9,11): total 5 hidden bytes. A point past both shifts
    // left by both lengths combined.
    let snap = SyntaxSnapshot::build(&[0], &[vec![(2, 5), (9, 11)]]);
    let sp = snap.buffer_to_syntax(BufferPoint { line: 0, col: 13 });
    assert_eq!(sp, SyntaxPoint { line: 0, col: 8 }); // 13 - 3 - 2 = 8
    let bp = snap.syntax_to_buffer(sp);
    assert_eq!(bp, BufferPoint { line: 0, col: 13 });
}

#[test]
fn hidden_byte_count_sums_only_the_named_lines_own_ranges() {
    let snap = SyntaxSnapshot::build(&[0, 20], &[vec![(2, 5)], vec![(22, 30)]]);
    assert_eq!(snap.hidden_byte_count(0), 3);
    assert_eq!(snap.hidden_byte_count(1), 8);
    assert_eq!(snap.hidden_byte_count(2), 0); // no such line: clamps to 0
}

#[test]
fn a_line_with_no_hidden_ranges_is_the_identity_conversion() {
    let snap = SyntaxSnapshot::build(&[0], &[vec![]]);
    for col in 0..10 {
        let bp = BufferPoint { line: 0, col };
        assert_eq!(snap.buffer_to_syntax(bp), SyntaxPoint { line: 0, col });
        assert_eq!(snap.syntax_to_buffer(SyntaxPoint { line: 0, col }), bp);
    }
}
