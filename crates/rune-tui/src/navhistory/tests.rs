#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    dead_code
)]

use std::num::NonZeroU64;

use super::*;

fn doc(n: u64) -> DocumentId {
    DocumentId(NonZeroU64::new(n).expect("nonzero"))
}

fn place(doc: DocumentId, offset: usize) -> Place {
    Place {
        doc,
        path: None,
        offset,
        kind: PlaceKind::Visited,
    }
}

#[test]
fn push_truncates_the_forward_tail() {
    let d = doc(1);
    let mut history = NavHistory::default();
    history.push(place(d, 0), false);
    history.push(place(d, 10), false);
    history.push(place(d, 20), false);
    history.current = 1;

    history.push(place(d, 99), false);

    assert_eq!(history.places, vec![place(d, 0), place(d, 99)]);
    assert_eq!(history.current, 2);
}

#[test]
fn replace_last_overwrites_instead_of_growing() {
    let d = doc(1);
    let mut history = NavHistory::default();
    history.push(place(d, 0), false);
    history.push(place(d, 10), false);

    history.push(place(d, 99), true);

    assert_eq!(history.places, vec![place(d, 0), place(d, 99)]);
    assert_eq!(history.current, 2);
}

#[test]
fn overflow_past_max_places_drops_oldest_and_keeps_current_at_the_tip() {
    let d = doc(1);
    let mut history = NavHistory::default();
    for offset in 0..MAX_PLACES + 5 {
        history.push(place(d, offset), false);
    }

    assert_eq!(history.places.len(), MAX_PLACES);
    assert_eq!(history.current, MAX_PLACES);
    assert_eq!(history.places.first(), Some(&place(d, 5)));
    assert_eq!(history.places.last(), Some(&place(d, MAX_PLACES + 4)));
}

#[test]
fn first_back_pushes_the_live_place_so_a_following_forward_returns_to_it() {
    let d = doc(1);
    let mut history = NavHistory::default();
    history.push(place(d, 0), false);
    let live = place(d, 50);

    let went_to = history.back(Some(live.clone()));

    assert_eq!(went_to, Some(place(d, 0)));
    assert_eq!(history.current, 0);
    assert_eq!(history.places, vec![place(d, 0), live.clone()]);

    let came_back = history.forward();

    assert_eq!(came_back, Some(live));
}

#[test]
fn forward_at_the_tip_returns_none() {
    let d = doc(1);
    let mut history = NavHistory::default();
    history.push(place(d, 0), false);

    assert_eq!(history.forward(), None);
}

#[test]
fn back_at_index_zero_returns_none_and_does_not_touch_the_live_place() {
    let mut history = NavHistory::default();

    let result = history.back(Some(place(doc(1), 50)));

    assert_eq!(result, None);
    assert!(history.places.is_empty());
}

#[test]
fn back_with_no_live_place_does_not_push_one() {
    let d = doc(1);
    let mut history = NavHistory::default();
    history.push(place(d, 0), false);

    let went_to = history.back(None);

    assert_eq!(went_to, Some(place(d, 0)));
    assert_eq!(history.places, vec![place(d, 0)]);
    assert!(!history.can_forward());
}

#[test]
fn shift_leaves_an_offset_before_the_edit_untouched() {
    let d = doc(1);
    let other = doc(2);
    let mut history = NavHistory::default();
    history.push(place(d, 5), false);
    history.push(place(other, 5), false);

    history.shift(d, 10, 5, 2);

    assert_eq!(history.places[0].offset, 5);
    assert_eq!(history.places[1].offset, 5);
}

#[test]
fn shift_moves_an_offset_after_the_edit_by_the_length_delta() {
    let d = doc(1);
    let mut history = NavHistory::default();
    history.push(place(d, 20), false);

    history.shift(d, 10, 5, 2);

    assert_eq!(history.places[0].offset, 17);
}

#[test]
fn shift_collapses_an_offset_inside_the_removed_span_to_start() {
    let d = doc(1);
    let mut history = NavHistory::default();
    history.push(place(d, 12), false);

    history.shift(d, 10, 5, 2);

    assert_eq!(history.places[0].offset, 10);
}

#[test]
fn drop_at_before_current_shifts_current_down() {
    let d = doc(1);
    let mut history = NavHistory::default();
    history.push(place(d, 0), false);
    history.push(place(d, 1), false);
    history.push(place(d, 2), false);

    history.drop_at(0);

    assert_eq!(history.places, vec![place(d, 1), place(d, 2)]);
    assert_eq!(history.current, history.places.len());
}

#[test]
fn drop_at_after_current_leaves_current_untouched() {
    let d = doc(1);
    let mut history = NavHistory::default();
    history.push(place(d, 0), false);
    history.push(place(d, 1), false);
    history.push(place(d, 2), false);
    history.current = 1;

    history.drop_at(2);

    assert_eq!(history.places, vec![place(d, 0), place(d, 1)]);
    assert_eq!(history.current, 1);
}
