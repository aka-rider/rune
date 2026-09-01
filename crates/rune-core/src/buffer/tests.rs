//! Tests for `buffer::mod`, split out to keep the owning module under the
//! 500-line budget.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;

#[test]
fn from_bytes() {
    let b = Buffer::from_bytes(b"Hello \xe2\x98\xba World".to_vec())
        .expect("valid utf-8 should not error");
    assert_eq!(b.content(), "Hello \u{263a} World");

    let err = Buffer::from_bytes(vec![0xff, 0xfe]);
    assert_eq!(err, Err(BufferError::InvalidUtf8));
}

#[test]
fn sorted_edits_validate_rejects_ascending_order() {
    let err = SortedEdits::validate(vec![
        Edit {
            start: 0,
            end: 5,
            insert: "a".to_string(),
        },
        Edit {
            start: 6,
            end: 11,
            insert: "b".to_string(),
        },
    ]);
    assert_eq!(err, Err(BufferError::EditsNotSortedOrOverlapping));
}

#[test]
fn sorted_edits_validate_rejects_overlap() {
    let err = SortedEdits::validate(vec![
        Edit {
            start: 5,
            end: 10,
            insert: "a".to_string(),
        },
        Edit {
            start: 0,
            end: 6,
            insert: "b".to_string(),
        },
    ]);
    assert_eq!(err, Err(BufferError::EditsNotSortedOrOverlapping));
}

#[test]
fn apply_edits_accepts_a_validated_descending_non_overlapping_batch() {
    let b = Buffer::new("hello world");
    let sorted = SortedEdits::validate(vec![
        Edit {
            start: 6,
            end: 11,
            insert: "b".to_string(),
        },
        Edit {
            start: 0,
            end: 5,
            insert: "a".to_string(),
        },
    ])
    .expect("already descending and non-overlapping");
    assert!(b.apply_edits(&sorted).is_ok());
}

#[test]
fn clone_and_sort_edits_descending_test() {
    let edits = vec![
        Edit {
            start: 0,
            end: 5,
            insert: "a".to_string(),
        },
        Edit {
            start: 6,
            end: 11,
            insert: "b".to_string(),
        },
    ];
    let sorted = clone_and_sort_edits_descending(&edits);

    assert_eq!(edits[0].start, 0, "original slice was mutated");
    assert_eq!(sorted[0].start, 6);
    assert_eq!(sorted[1].start, 0);
}

#[test]
fn apply_edits_rejects_a_batch_whose_edits_collide_on_post_edit_start() {
    let b = Buffer::new("ab");
    let sorted = SortedEdits::sort(&[
        Edit {
            start: 1,
            end: 2,
            insert: String::new(),
        },
        Edit {
            start: 0,
            end: 1,
            insert: String::new(),
        },
    ]);
    let err = b.apply_edits(&sorted);
    assert_eq!(err, Err(BufferError::DuplicateEditStart { start: 0 }));
}

#[test]
fn check_char_boundary_rejects_a_mid_rune_offset() {
    let content = "h\u{e9}llo";
    assert_eq!(check_char_boundary(content, 0), Ok(()));
    assert_eq!(
        check_char_boundary(content, 2),
        Err(BufferError::SplitsRune { offset: 2 })
    );
}

#[test]
fn clamp_to_char_boundary_snaps_down_and_clamps_to_content_len() {
    let content = "h\u{e9}llo";
    assert_eq!(content.len(), 6);
    assert_eq!(clamp_to_char_boundary(content, 3), 3);
    assert_eq!(clamp_to_char_boundary(content, 2), 1);
    assert_eq!(clamp_to_char_boundary(content, 100), 6);
}

#[test]
fn buffer_error_display_messages_are_never_empty() {
    let cases = [
        BufferError::InvalidUtf8,
        BufferError::EditsNotSortedOrOverlapping,
        BufferError::OutOfBounds {
            start: 1,
            end: 2,
            len: 3,
        },
        BufferError::SplitsRune { offset: 4 },
        BufferError::DuplicateEditStart { start: 5 },
    ];
    for case in cases {
        assert!(
            !case.to_string().is_empty(),
            "{case:?} must render a user-facing message"
        );
    }
    assert_eq!(
        BufferError::OutOfBounds {
            start: 1,
            end: 2,
            len: 3
        }
        .to_string(),
        "edit out of bounds: [1,2) len=3"
    );
}

#[test]
fn is_empty_reflects_content() {
    assert!(Buffer::new("").is_empty());
    assert!(!Buffer::new("x").is_empty());
}

#[test]
fn version_starts_at_one_and_increments_per_edit() {
    let b = Buffer::new("hi");
    assert_eq!(b.version(), 1);
    let edited = b.insert(0, "x").expect("edit applies");
    assert_eq!(edited.version(), 2);
}

#[test]
fn slice_returns_the_requested_range_and_none_out_of_bounds() {
    let b = Buffer::new("hello world");
    assert_eq!(b.slice(0, 5), Some("hello"));
    assert_eq!(b.slice(6, 11), Some("world"));
    assert_eq!(b.slice(0, 100), None);
}

#[test]
fn byte_returns_the_byte_at_offset_and_none_out_of_bounds() {
    let b = Buffer::new("hi");
    assert_eq!(b.byte(0), Some(b'h'));
    assert_eq!(b.byte(1), Some(b'i'));
    assert_eq!(b.byte(2), None);
}

#[test]
fn rune_at_returns_the_char_and_its_byte_length() {
    let b = Buffer::new("h\u{263a}i");
    assert_eq!(b.rune_at(0), Some(('h', 1)));
    assert_eq!(b.rune_at(1), Some(('\u{263a}', 3)));
    assert_eq!(b.rune_at(100), None);
}

#[test]
fn delete_removes_the_range() {
    let b = Buffer::new("hello world");
    let deleted = b.delete(5, 11).expect("delete applies");
    assert_eq!(deleted.content(), "hello");
}

#[test]
fn applied_edit_end_is_post_edit_start_plus_insert_len() {
    let b = Buffer::new("0123456789");
    let (_, applied) = b
        .apply_edits(&SortedEdits::single(Edit {
            start: 5,
            end: 5,
            insert: "abc".to_string(),
        }))
        .expect("insert applies");
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].start, 5);
    assert_eq!(applied[0].end, 8);
}

/// A single edit whose `start` exceeds its `end` fails on the descending
/// `e.start > e.end` half of `validate_edit_batch`'s bounds check while
/// `e.end > len` is false — this catches both the whole-function
/// `-> Ok(())` mutant (which lets the batch fall through to
/// `check_char_boundary`, producing `SplitsRune` instead) and the
/// `||` -> `&&` mutant on the same condition (which also skips the early
/// return here since only one half of the pair is true).
#[test]
fn validate_edit_batch_rejects_start_past_end_even_when_end_is_in_bounds() {
    let b = Buffer::new("hello");
    let err = b.replace(10, 3, "");
    assert_eq!(
        err,
        Err(BufferError::OutOfBounds {
            start: 10,
            end: 3,
            len: 5
        })
    );
}

#[test]
fn sorted_edits_is_empty_and_len() {
    let empty = SortedEdits::sort(&[]);
    assert!(empty.is_empty());
    assert_eq!(empty.len(), 0);

    let one = SortedEdits::single(Edit {
        start: 0,
        end: 0,
        insert: "x".to_string(),
    });
    assert!(!one.is_empty());
    assert_eq!(one.len(), 1);
}
