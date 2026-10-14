#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use rune_core::coords::{BufferOffset, VisualCol};
use rune_core::cursor::CursorId;

fn cursor(n: u32, anchor: usize, position: usize) -> Cursor {
    Cursor {
        anchor: BufferOffset(anchor),
        position: BufferOffset(position),
        desired_col: VisualCol(0),
        id: CursorId::try_from(n).expect("test ids are non-zero"),
    }
}

fn lines_of(blocks: &[LineBlock]) -> Vec<(usize, usize)> {
    blocks
        .iter()
        .map(|b| (b.lines.start, b.lines.end))
        .collect()
}

fn ids_of(block: &LineBlock) -> Vec<u32> {
    block.cursors.iter().map(|c| c.id.get()).collect()
}

#[test]
fn a_reversed_selection_covers_the_same_lines_as_a_forward_one() {
    let buf = Buffer::new("a\nb\nc\nd");
    assert_eq!(selected_lines(&cursor(1, 5, 1), &buf), 0..3);
    assert_eq!(selected_lines(&cursor(1, 1, 5), &buf), 0..3);
}

#[test]
fn a_multi_line_selection_ending_at_column_zero_excludes_that_line() {
    let buf = Buffer::new("a\nb\nc");
    assert_eq!(selected_lines(&cursor(1, 0, 2), &buf), 0..1);
    assert_eq!(selected_lines(&cursor(1, 2, 0), &buf), 0..1);
    assert_eq!(selected_lines(&cursor(1, 0, 3), &buf), 0..2);
}

#[test]
fn a_selection_ending_at_column_zero_of_an_empty_last_line_excludes_it() {
    let buf = Buffer::new("a\n");
    assert_eq!(selected_lines(&cursor(1, 0, 2), &buf), 0..1);
}

#[test]
fn a_caret_at_column_zero_selects_its_own_line() {
    let buf = Buffer::new("a\nb\nc");
    assert_eq!(selected_lines(&cursor(1, 2, 2), &buf), 1..2);
    assert_eq!(selected_lines(&cursor(1, 0, 0), &buf), 0..1);
}

#[test]
fn an_empty_buffer_has_one_line() {
    let buf = Buffer::new("");
    assert_eq!(selected_lines(&cursor(1, 0, 0), &buf), 0..1);
    assert_eq!(
        lines_of(&overlapping_line_blocks(&[cursor(1, 0, 0)], &buf)),
        [(0, 1)]
    );
}

#[test]
fn a_last_line_without_a_terminator_is_selectable() {
    let buf = Buffer::new("a\nb");
    assert_eq!(selected_lines(&cursor(1, 3, 3), &buf), 1..2);
    assert_eq!(selected_lines(&cursor(1, 0, 3), &buf), 0..2);
}

#[test]
fn crlf_line_breaks_are_one_line_boundary_each() {
    let buf = Buffer::new("a\r\nb\r\nc");
    assert_eq!(selected_lines(&cursor(1, 0, 2), &buf), 0..1);
    assert_eq!(selected_lines(&cursor(1, 0, 3), &buf), 0..1);
    assert_eq!(selected_lines(&cursor(1, 0, 4), &buf), 0..2);
}

#[test]
fn two_carets_on_one_line_form_one_block_either_way() {
    let buf = Buffer::new("abc\ndef");
    let carets = [cursor(1, 2, 2), cursor(2, 0, 0)];
    for blocks in [
        overlapping_line_blocks(&carets, &buf),
        touching_line_blocks(&carets, &buf),
    ] {
        assert_eq!(lines_of(&blocks), [(0, 1)]);
        assert_eq!(
            ids_of(&blocks[0]),
            [1, 2],
            "both carets belong to the block"
        );
    }
}

#[test]
fn adjacent_lines_are_separate_blocks_when_overlapping_but_one_when_touching() {
    let buf = Buffer::new("a\nb\nc\nd");
    let carets = [cursor(1, 0, 0), cursor(2, 2, 2)];
    assert_eq!(
        lines_of(&overlapping_line_blocks(&carets, &buf)),
        [(0, 1), (1, 2)]
    );
    let touching = touching_line_blocks(&carets, &buf);
    assert_eq!(lines_of(&touching), [(0, 2)]);
    assert_eq!(ids_of(&touching[0]), [1, 2]);
}

#[test]
fn lines_with_a_gap_stay_separate_blocks_under_touching() {
    let buf = Buffer::new("a\nb\nc\nd");
    let carets = [cursor(1, 0, 0), cursor(2, 4, 4)];
    assert_eq!(
        lines_of(&touching_line_blocks(&carets, &buf)),
        [(0, 1), (2, 3)]
    );
}

#[test]
fn overlapping_selections_merge_into_the_union_of_their_lines() {
    let buf = Buffer::new("a\nb\nc\nd\ne");
    let selections = [cursor(1, 0, 5), cursor(2, 4, 7)];
    let blocks = overlapping_line_blocks(&selections, &buf);
    assert_eq!(lines_of(&blocks), [(0, 4)]);
    assert_eq!(ids_of(&blocks[0]), [1, 2]);
}

#[test]
fn blocks_come_out_in_document_order_whatever_the_cursor_order() {
    let buf = Buffer::new("a\nb\nc\nd\ne");
    let carets = [cursor(1, 8, 8), cursor(2, 0, 0), cursor(3, 4, 4)];
    let blocks = touching_line_blocks(&carets, &buf);
    assert_eq!(lines_of(&blocks), [(0, 1), (2, 3), (4, 5)]);
    assert_eq!(ids_of(&blocks[0]), [2]);
}

#[test]
fn a_selection_ending_at_column_zero_does_not_touch_the_next_caret_line_when_overlapping() {
    let buf = Buffer::new("a\nb\nc\nd");
    let carets = [cursor(1, 0, 2), cursor(2, 2, 2)];
    assert_eq!(
        lines_of(&overlapping_line_blocks(&carets, &buf)),
        [(0, 1), (1, 2)]
    );
}
