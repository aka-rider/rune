//! Unit tests for the edit-batch commit chokepoint, kept in a sibling
//! file so the chokepoint itself stays inside the 500-line budget.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;

pub(super) fn cid(n: u32) -> CursorId {
    CursorId::try_from(n).expect("test ids are non-zero")
}

/// Two independent, non-touching CURSORS — `CursorSet::merge` correctly
/// leaves positions 0 and 1 separate, since neither's (zero-width)
/// selection touches the other's — each derive a one-byte Delete-
/// forward range from their own position: `[0,1)` and `[1,2)`. Those
/// two DERIVED ranges do touch. Left unmerged, `Buffer::apply_edits`
/// hands back two `AppliedEdit`s that both land on post-edit `start ==
/// 0` — the exact illegal state `undo::reapply`'s precondition assert
/// exists to catch (`crates/rune-fuzz` artifact `no-panic-7f29861c`,
/// checked in as `repros/no-panic-01.rune`).
#[test]
fn merges_two_adjacent_bare_deletes() {
    let infos = vec![
        (
            Edit {
                start: 1,
                end: 2,
                insert: String::new(),
            },
            cid(2),
        ),
        (
            Edit {
                start: 0,
                end: 1,
                insert: String::new(),
            },
            cid(1),
        ),
    ];
    let merged = coalesce_touching_edits(infos);
    assert_eq!(
        merged.len(),
        1,
        "touching ranges must collapse into one edit"
    );
    assert_eq!(
        merged.first(),
        Some(&(
            Edit {
                start: 0,
                end: 2,
                insert: String::new(),
            },
            cid(1),
        )),
        "the lower cursor id survives, matching CursorSet::merge's own rule"
    );
}

/// Two cursors sitting inside the same word: a delete-word-right from
/// each derives OVERLAPPING (not just touching) ranges `[0,5)` and
/// `[2,7)`. `Buffer::apply_edits` would otherwise reject this batch
/// outright as `EditsNotSortedOrOverlapping` — a spurious "edit
/// failed" for an entirely ordinary multi-cursor action.
#[test]
fn merges_overlapping_word_deletes() {
    let infos = vec![
        (
            Edit {
                start: 2,
                end: 7,
                insert: String::new(),
            },
            cid(9),
        ),
        (
            Edit {
                start: 0,
                end: 5,
                insert: String::new(),
            },
            cid(3),
        ),
    ];
    let merged = coalesce_touching_edits(infos);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged.first().map(|(e, _)| (e.start, e.end)), Some((0, 7)));
}

/// A real gap between two cursors' ranges must survive untouched —
/// the common case (most multi-cursor edits do not collide at all).
#[test]
fn leaves_genuinely_separated_edits_alone() {
    let infos = vec![
        (
            Edit {
                start: 5,
                end: 6,
                insert: String::new(),
            },
            cid(2),
        ),
        (
            Edit {
                start: 0,
                end: 1,
                insert: String::new(),
            },
            cid(1),
        ),
    ];
    let merged = coalesce_touching_edits(infos);
    assert_eq!(merged.len(), 2, "a real gap must not be merged away");
}

#[test]
fn two_cursors_on_the_byte_identical_edit_coalesce_to_one() {
    let infos = vec![
        (
            Edit {
                start: 0,
                end: 5,
                insert: "HELLO".to_string(),
            },
            cid(2),
        ),
        (
            Edit {
                start: 0,
                end: 5,
                insert: "HELLO".to_string(),
            },
            cid(1),
        ),
    ];
    let merged = coalesce_touching_edits(infos);
    assert_eq!(
        merged,
        vec![(
            Edit {
                start: 0,
                end: 5,
                insert: "HELLO".to_string(),
            },
            cid(1),
        )],
        "two cursors landing on the byte-identical edit must collapse to one, \
         surviving as the lower cursor id"
    );
}

#[test]
fn first_overlap_start_finds_a_genuine_overlap_but_not_a_touching_pair() {
    let touching = vec![
        (
            Edit {
                start: 0,
                end: 2,
                insert: String::new(),
            },
            cid(1),
        ),
        (
            Edit {
                start: 2,
                end: 4,
                insert: "x".to_string(),
            },
            cid(2),
        ),
    ];
    assert_eq!(first_overlap_start(&touching), None);

    let overlapping = vec![
        (
            Edit {
                start: 0,
                end: 5,
                insert: "HELLO".to_string(),
            },
            cid(1),
        ),
        (
            Edit {
                start: 3,
                end: 8,
                insert: "WORLD".to_string(),
            },
            cid(2),
        ),
    ];
    assert_eq!(first_overlap_start(&overlapping), Some(3));
}
