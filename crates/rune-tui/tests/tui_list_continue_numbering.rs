#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod tui_edit_common;

use rune_tui::keymap::{KeyCode, Mods};

use tui_edit_common::{enter, marked, press, undo, with_carets};

#[test]
fn outdenting_an_empty_nested_item_renumbers_the_parents_following_siblings() {
    let mut app = with_carets("1. a\n   - |\n2. b");
    enter(&mut app);
    assert_eq!(marked(&mut app), "1. a\n2. |\n3. b");
}

#[test]
fn enter_on_an_ordered_item_numbers_the_next_one_and_renumbers_the_rest() {
    let mut app = with_carets("1. a|\n2. b\n3. c");
    enter(&mut app);
    assert_eq!(marked(&mut app), "1. a\n2. |\n3. b\n4. c");
}

#[test]
fn renumbering_stops_at_the_first_gap_in_the_run() {
    let mut app = with_carets("1. a|\n2. b\n5. c");
    enter(&mut app);
    assert_eq!(marked(&mut app), "1. a\n2. |\n3. b\n5. c");
}

#[test]
fn enter_on_a_paren_delimited_item_keeps_the_delimiter() {
    let mut app = with_carets("1) a|");
    enter(&mut app);
    assert_eq!(marked(&mut app), "1) a\n2) |");
}

#[test]
fn one_undo_removes_the_newline_the_marker_and_the_renumbering_together() {
    let mut app = with_carets("1. a|\n2. b\n3. c");
    enter(&mut app);
    assert_eq!(marked(&mut app), "1. a\n2. |\n3. b\n4. c");
    undo(&mut app);
    assert_eq!(marked(&mut app), "1. a|\n2. b\n3. c");
}

#[test]
fn one_undo_restores_a_quote_exit_that_edited_two_lines() {
    let mut app = with_carets("> a\n>\n> |");
    enter(&mut app);
    assert_eq!(marked(&mut app), "> a\n\n|");
    undo(&mut app);
    assert_eq!(marked(&mut app), "> a\n>\n> |");
}

#[test]
fn two_carets_in_one_ordered_list_number_every_item_once() {
    let mut app = with_carets("1. a|\n2. b|\n3. c");
    enter(&mut app);
    assert_eq!(marked(&mut app), "1. a\n2. |\n3. b\n4. |\n5. c");
}

#[test]
fn two_carets_on_separate_bullets_each_continue() {
    let mut app = with_carets("- a|\n- b|");
    enter(&mut app);
    assert_eq!(marked(&mut app), "- a\n- |\n- b\n- |");
}

#[test]
fn a_run_is_cut_where_another_caret_removes_the_next_item_so_numbering_stays_gap_free() {
    let mut app = with_carets("1. a|\n2. |\n3. c");
    enter(&mut app);
    assert_eq!(marked(&mut app), "1. a\n2. |\n|\n3. c");
    undo(&mut app);
    assert_eq!(marked(&mut app), "1. a|\n2. |\n3. c");
}

#[test]
fn a_caret_at_column_zero_of_the_next_numbered_line_leaves_that_line_renumbered() {
    let mut app = with_carets("1. a|\n|2. b");
    enter(&mut app);
    assert_eq!(marked(&mut app), "1. a\n2. |\n\n|3. b");
}

#[test]
fn a_caret_inside_the_digits_of_a_following_line_cuts_the_run_without_an_error() {
    let mut app = with_carets("9. a|\n1|0. b\n11. c");
    let posts_before = rune_tui::messages::posts(&app);
    enter(&mut app);
    assert_eq!(marked(&mut app), "9. a\n10. |\n1\n|0. b\n11. c");
    assert_eq!(rune_tui::messages::posts(&app), posts_before);
}

#[test]
fn two_carets_on_one_ordered_line_number_their_new_items_consecutively() {
    let mut app = with_carets("1. a|b|c\n2. d");
    enter(&mut app);
    assert_eq!(marked(&mut app), "1. a\n2. |b\n3. |c\n4. d");
}

#[test]
fn two_carets_on_one_ordered_line_before_a_numbering_gap_do_not_touch_the_far_side() {
    let mut app = with_carets("1. a|b|c\n3. d");
    enter(&mut app);
    assert_eq!(marked(&mut app), "1. a\n2. |b\n3. |c\n3. d");
}

#[test]
fn an_outdent_takes_the_number_after_an_item_another_caret_inserted_above_it() {
    let mut app = with_carets("1. a|\n   - |\n2. b");
    enter(&mut app);
    assert_eq!(marked(&mut app), "1. a\n2. |\n3. |\n4. b");
}

#[test]
fn an_outdent_takes_the_number_after_an_item_inserted_above_it_even_without_following_siblings() {
    let mut app = with_carets("1. a|\n   - |");
    enter(&mut app);
    assert_eq!(marked(&mut app), "1. a\n2. |\n3. |");
}

#[test]
fn two_adjacent_exiting_carets_share_the_quote_prefix_deletion_between_them() {
    let mut app = with_carets("> a\n>\n> |\n> |");
    enter(&mut app);
    assert_eq!(marked(&mut app), "> a\n\n|\n|");
}

#[test]
fn a_conflict_that_is_not_a_duplicate_is_reported_and_leaves_the_text_alone() {
    let mut app = with_carets("> |\n> |");
    let posts_before = rune_tui::messages::posts(&app);
    enter(&mut app);
    assert_eq!(marked(&mut app), "> |\n> |");
    assert!(rune_tui::messages::posts(&app) > posts_before);
    assert!(
        rune_tui::messages::newest_text(&app).is_some_and(|m| m.contains("overlapping")),
        "expected an overlap error, got {:?}",
        rune_tui::messages::newest_text(&app)
    );
}

#[test]
fn backspace_on_two_bare_markers_removes_both_in_one_undo_step() {
    let mut app = with_carets("- a\n- |\n- b\n1. |");
    press(&mut app, KeyCode::Backspace, Mods::NONE);
    assert_eq!(marked(&mut app), "- a\n|\n- b\n|");
    undo(&mut app);
    assert_eq!(marked(&mut app), "- a\n- |\n- b\n1. |");
}
