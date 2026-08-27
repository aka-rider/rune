//! Tests for `cursor`, split out to keep the owning module under the
//! 500-line budget.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;

fn id(n: u32) -> CursorId {
    CursorId::try_from(n).expect("test ids are non-zero")
}

#[test]
fn new_single_cursor_has_id_one() {
    let cs = CursorSet::new(5);
    assert_eq!(cs.len(), 1);
    assert_eq!(cs.primary().position, BufferOffset(5));
    assert_eq!(cs.primary().id, CursorId::FIRST);
}

#[test]
fn merge_coalesces_overlapping_selections() {
    let a = Cursor {
        position: BufferOffset(5),
        anchor: BufferOffset(0),
        desired_col: VisualCol(0),
        id: id(1),
    };
    let b = Cursor {
        position: BufferOffset(3),
        anchor: BufferOffset(8),
        desired_col: VisualCol(0),
        id: id(2),
    };
    let cs = CursorSet::new_from(&[a, b]);
    assert_eq!(cs.len(), 1);
    let merged = cs.primary();
    assert_eq!(
        (merged.selection_start(), merged.selection_end()),
        (BufferOffset(0), BufferOffset(8))
    );
}

/// An empty cursor has no direction — `reversed()` is false for it
/// however the user got there. Taking the merged direction from an
/// empty survivor therefore flips a real selection to face the wrong
/// way: pressing Up merges a clamped-at-top empty cursor with a
/// selection reaching up to it, and the head must stay at the TOP.
#[test]
fn merge_takes_its_direction_from_the_cursor_that_has_a_selection() {
    let clamped_at_top = Cursor {
        position: BufferOffset(0),
        anchor: BufferOffset(0),
        desired_col: VisualCol(0),
        id: id(1),
    };
    let reaching_up = Cursor {
        position: BufferOffset(0),
        anchor: BufferOffset(8),
        desired_col: VisualCol(0),
        id: id(2),
    };
    let cs = CursorSet::new_from(&[clamped_at_top, reaching_up]);
    assert_eq!(cs.len(), 1);
    let merged = cs.primary();
    assert_eq!(
        (merged.selection_start(), merged.selection_end()),
        (BufferOffset(0), BufferOffset(8))
    );
    assert_eq!(
        merged.position,
        BufferOffset(0),
        "merging an empty cursor with a selection reaching up must keep the head at the top"
    );
}

/// [rune-core 14]: when two overlapping cursors merge, the survivor's
/// `reversed()` flag must come from whichever of the two carries the
/// surviving (lower) id — not always the earlier-sorted cursor.
#[test]
fn merge_survivor_keeps_the_reversed_flag_of_the_lower_id_cursor() {
    // `a` (id 1, survivor) is NOT reversed: position is its selection end.
    let a = Cursor {
        position: BufferOffset(8),
        anchor: BufferOffset(0),
        desired_col: VisualCol(0),
        id: id(1),
    };
    // `b` (id 2) IS reversed and sorts first by selection_start.
    let b = Cursor {
        position: BufferOffset(3),
        anchor: BufferOffset(6),
        desired_col: VisualCol(0),
        id: id(2),
    };
    let merged = CursorSet::new_from(&[a, b]).primary();
    assert_eq!(merged.id, id(1), "lower id survives");
    assert!(
        !merged.reversed(),
        "the survivor's own reversed flag (id 1, not reversed) must win, \
         not the other cursor's"
    );
    assert_eq!(
        (merged.selection_start(), merged.selection_end()),
        (BufferOffset(0), BufferOffset(8))
    );
}

#[test]
fn new_from_specs_assigns_distinct_fresh_ids() {
    let specs = [
        CursorSpec {
            position: BufferOffset(0),
            anchor: BufferOffset(0),
            desired_col: VisualCol(0),
        },
        CursorSpec {
            position: BufferOffset(20),
            anchor: BufferOffset(20),
            desired_col: VisualCol(0),
        },
    ];
    let cs = CursorSet::new_from_specs(&specs);
    assert_eq!(cs.len(), 2);
    let ids: Vec<CursorId> = cs.all().iter().map(|c| c.id).collect();
    assert_ne!(ids[0], ids[1], "each spec gets a distinct id");
}

/// [rune-core 14]: `new_from` does not itself deduplicate ids — two
/// cursors sharing a non-zero id both survive `new_from` unless their
/// selections happen to touch (`merge`'s job, not id assignment's).
#[test]
fn new_from_does_not_dedupe_non_touching_duplicate_ids() {
    let a = Cursor {
        position: BufferOffset(0),
        anchor: BufferOffset(0),
        desired_col: VisualCol(0),
        id: id(7),
    };
    let b = Cursor {
        position: BufferOffset(50),
        anchor: BufferOffset(50),
        desired_col: VisualCol(0),
        id: id(7),
    };
    let cs = CursorSet::new_from(&[a, b]);
    assert_eq!(cs.len(), 2, "non-touching cursors are not merged by id");
    let ids: Vec<CursorId> = cs.all().iter().map(|c| c.id).collect();
    assert_eq!(ids, vec![id(7), id(7)]);
}

#[test]
fn cursor_id_get_returns_the_stored_value() {
    assert_eq!(CursorId::FIRST.get(), 1);
    assert_eq!(id(5).get(), 5);
}

#[test]
fn cursor_id_display_renders_the_number() {
    assert_eq!(id(5).to_string(), "5");
}

#[test]
fn cursor_id_zero_display_is_a_nonempty_message() {
    assert_eq!(CursorIdZero.to_string(), "cursor id must be non-zero");
}

#[test]
fn selection_range_normalizes_a_reversed_selection() {
    let c = Cursor {
        position: BufferOffset(2),
        anchor: BufferOffset(8),
        desired_col: VisualCol(0),
        id: CursorId::FIRST,
    };
    assert_eq!(c.selection_range(), (BufferOffset(2), BufferOffset(8)));
}

#[test]
fn selection_range_leaves_a_forward_selection_as_is() {
    let c = Cursor {
        position: BufferOffset(8),
        anchor: BufferOffset(2),
        desired_col: VisualCol(0),
        id: CursorId::FIRST,
    };
    assert_eq!(c.selection_range(), (BufferOffset(2), BufferOffset(8)));
}

#[test]
fn reversed_is_false_when_position_equals_anchor() {
    let c = Cursor {
        position: BufferOffset(5),
        anchor: BufferOffset(5),
        desired_col: VisualCol(0),
        id: CursorId::FIRST,
    };
    assert!(!c.reversed());
}

#[test]
fn reversed_is_true_when_position_precedes_anchor() {
    let c = Cursor {
        position: BufferOffset(2),
        anchor: BufferOffset(8),
        desired_col: VisualCol(0),
        id: CursorId::FIRST,
    };
    assert!(c.reversed());
}

#[test]
fn new_from_continues_ids_past_the_highest_existing_one() {
    let a = Cursor {
        position: BufferOffset(0),
        anchor: BufferOffset(0),
        desired_col: VisualCol(0),
        id: id(1),
    };
    let b = Cursor {
        position: BufferOffset(10),
        anchor: BufferOffset(10),
        desired_col: VisualCol(0),
        id: id(3),
    };
    let cs = CursorSet::new_from(&[a, b]);
    let added = cs.add(CursorSpec {
        position: BufferOffset(20),
        anchor: BufferOffset(20),
        desired_col: VisualCol(0),
    });
    let new_cursor = added
        .all()
        .iter()
        .find(|c| c.position == BufferOffset(20))
        .expect("added cursor present");
    assert_eq!(
        new_cursor.id,
        id(4),
        "next id must continue past the highest existing id, not restart at 1"
    );
}

#[test]
fn new_from_positions_builds_one_cursor_per_position() {
    let cs = CursorSet::new_from_positions(&[3, 7]);
    assert_eq!(cs.len(), 2);
    let positions: Vec<usize> = cs.all().iter().map(|c| c.position.get()).collect();
    assert_eq!(positions, vec![3, 7]);
}

#[test]
fn cursor_set_is_never_empty() {
    assert!(!CursorSet::new(0).is_empty());
}

#[test]
fn is_multi_is_false_for_one_cursor_true_for_two() {
    assert!(!CursorSet::new(0).is_multi());
    let multi = CursorSet::new_from_positions(&[0, 50]);
    assert!(multi.is_multi());
}

#[test]
fn add_appends_a_new_cursor() {
    let cs = CursorSet::new(0);
    let added = cs.add(CursorSpec {
        position: BufferOffset(50),
        anchor: BufferOffset(50),
        desired_col: VisualCol(0),
    });
    assert_eq!(added.len(), 2);
}

#[test]
fn collapse_to_keeps_only_the_given_cursor() {
    let cs = CursorSet::new_from_positions(&[1, 5, 9]);
    let target = *cs
        .all()
        .iter()
        .find(|c| c.position == BufferOffset(5))
        .expect("cursor at 5 exists");
    let collapsed = cs.collapse_to(target);
    assert_eq!(collapsed.len(), 1);
    assert_eq!(collapsed.primary().position, BufferOffset(5));
}

/// Two cursors sharing the same selection start but a DIFFERENT end sort
/// by that end (ascending) when the start ties, before the merge loop
/// ever runs — but the merged cursor's `desired_col` comes from the
/// survivor (the lower id), not from whichever cursor the sort happened
/// to place first.
#[test]
fn merge_orders_same_start_cursors_by_end_not_by_id() {
    let short_selection = Cursor {
        position: BufferOffset(8),
        anchor: BufferOffset(5),
        desired_col: VisualCol(100),
        id: id(2),
    };
    let long_selection = Cursor {
        position: BufferOffset(20),
        anchor: BufferOffset(5),
        desired_col: VisualCol(200),
        id: id(1),
    };
    let merged = CursorSet::new_from(&[short_selection, long_selection]).primary();
    assert_eq!(
        merged.desired_col,
        VisualCol(200),
        "the lower-id survivor's desired_col must survive the merge, \
         not the earlier-by-end cursor's"
    );
}

/// [rune-core 14]: mirrors `merge_survivor_keeps_the_reversed_flag_of_the_
/// lower_id_cursor` for `desired_col` — the merged cursor's `desired_col`
/// must come from the survivor (lower id), not whichever cursor the merge
/// loop happened to be holding as `current`.
#[test]
fn merge_survivor_keeps_the_desired_col_of_the_lower_id_cursor() {
    let a = Cursor {
        position: BufferOffset(8),
        anchor: BufferOffset(0),
        desired_col: VisualCol(100),
        id: id(1),
    };
    let b = Cursor {
        position: BufferOffset(3),
        anchor: BufferOffset(6),
        desired_col: VisualCol(200),
        id: id(2),
    };
    let merged = CursorSet::new_from(&[a, b]).primary();
    assert_eq!(merged.id, id(1), "lower id survives");
    assert_eq!(
        merged.desired_col,
        VisualCol(100),
        "the survivor's own desired_col (id 1) must win, not the other cursor's"
    );
}

#[test]
fn map_transforms_every_cursor() {
    let cs = CursorSet::new(5);
    let mapped = cs.map(|c| Cursor {
        position: c.position + 1,
        anchor: c.position + 1,
        ..c
    });
    assert_eq!(mapped.primary().position, BufferOffset(6));
}
