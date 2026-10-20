#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use crate::commands::edit::undo;
use crate::commands::edit_lines::{delete_line, indent, outdent};
use crate::commands::test_support::{cursors, selecting};
use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_vfs::Mem;
use std::sync::Arc;

fn app_with(content: &str, cursor_offset: usize) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    let id = app.active;
    app.doc_mut(id)
        .unwrap()
        .set_cursors(CursorSet::new(cursor_offset.min(content.len())));
    app.doc_mut(id).unwrap().viewport.set_size(80, 23);
    app
}

fn selected(content: &str, anchor: usize, position: usize) -> App {
    let mut app = app_with(content, 0);
    let id = app.active;
    selecting(&mut app, id, anchor, position);
    app
}

fn text(app: &App) -> String {
    app.doc(app.active).unwrap().buffer().content().to_string()
}

fn selections(app: &App) -> Vec<(usize, usize)> {
    app.doc(app.active)
        .unwrap()
        .cursors()
        .all()
        .iter()
        .map(|c| (c.anchor.get(), c.position.get()))
        .collect()
}

fn journal_len(app: &App) -> usize {
    app.doc(app.active).unwrap().journal.len()
}

const FOUR_LINES: &str = "a\nb\nc\nd";

#[test]
fn clone_line_up_inserts_a_copy_above_and_the_caret_stays_on_the_copy() {
    let mut app = app_with("one\ntwo", "one\n".len() + 1);
    let id = app.active;
    clone_line_up(&mut app, id);
    assert_eq!(text(&app), "one\ntwo\ntwo");
    assert_eq!(selections(&app), [(5, 5)]);
}

#[test]
fn clone_line_up_works_on_the_first_line() {
    let mut app = app_with("only", 1);
    let id = app.active;
    clone_line_up(&mut app, id);
    assert_eq!(text(&app), "only\nonly");
    assert_eq!(selections(&app), [(1, 1)]);
}

#[test]
fn clone_line_down_inserts_a_copy_below_and_the_caret_moves_to_the_original() {
    let mut app = app_with("one\ntwo", 1);
    let id = app.active;
    clone_line_down(&mut app, id);
    assert_eq!(text(&app), "one\none\ntwo");
    assert_eq!(selections(&app), [(5, 5)]);
}

#[test]
fn clone_line_down_of_a_selection_copies_every_selected_line_and_selects_the_original() {
    let mut app = selected(FOUR_LINES, 0, 5);
    let id = app.active;
    clone_line_down(&mut app, id);
    assert_eq!(text(&app), "a\nb\nc\na\nb\nc\nd");
    assert_eq!(selections(&app), [(6, 11)]);
}

#[test]
fn clone_line_up_of_a_selection_copies_every_selected_line_and_keeps_the_selection_on_the_copy() {
    let mut app = selected(FOUR_LINES, 0, 5);
    let id = app.active;
    clone_line_up(&mut app, id);
    assert_eq!(text(&app), "a\nb\nc\na\nb\nc\nd");
    assert_eq!(selections(&app), [(0, 5)]);
}

#[test]
fn cloning_a_reversed_selection_keeps_its_direction() {
    let mut app = selected(FOUR_LINES, 5, 0);
    let id = app.active;
    clone_line_down(&mut app, id);
    assert_eq!(selections(&app), [(11, 6)]);
}

#[test]
fn cloning_a_selection_that_ends_at_column_zero_leaves_that_line_alone() {
    let mut app = selected(FOUR_LINES, 0, 4);
    let id = app.active;
    clone_line_down(&mut app, id);
    assert_eq!(text(&app), "a\nb\na\nb\nc\nd");
    assert_eq!(selections(&app), [(4, 8)]);
}

#[test]
fn cloning_the_last_unterminated_line_terminates_the_copy() {
    let mut app = app_with("a\nb", 2);
    let id = app.active;
    clone_line_down(&mut app, id);
    assert_eq!(text(&app), "a\nb\nb");
    assert_eq!(selections(&app), [(4, 4)]);
}

#[test]
fn cloning_the_crlf_files_last_unterminated_line_terminates_the_copy_with_crlf() {
    let mut up = app_with("A\r\nB", "A\r\n".len());
    let id = up.active;
    clone_line_up(&mut up, id);
    assert_eq!(text(&up), "A\r\nB\r\nB");

    let mut down = app_with("A\r\nB", "A\r\n".len());
    let id = down.active;
    clone_line_down(&mut down, id);
    assert_eq!(text(&down), "A\r\nB\r\nB");
}

#[test]
fn clone_line_down_preserves_a_crlf_terminator_verbatim() {
    let mut app = app_with("A\r\nB\r\nC", 0);
    let id = app.active;
    clone_line_down(&mut app, id);
    assert_eq!(text(&app), "A\r\nA\r\nB\r\nC");
}

#[test]
fn two_carets_on_one_line_clone_it_once_and_both_survive() {
    let mut app = app_with("one\ntwo", 0);
    let id = app.active;
    cursors(&mut app, id, &[(4, 4), (7, 7)]);
    clone_line_down(&mut app, id);
    assert_eq!(text(&app), "one\ntwo\ntwo");
    assert_eq!(selections(&app), [(8, 8), (11, 11)]);
}

#[test]
fn carets_on_adjacent_lines_each_get_their_own_copy() {
    let mut app = app_with("a\nb", 0);
    let id = app.active;
    cursors(&mut app, id, &[(0, 0), (2, 2)]);
    clone_line_down(&mut app, id);
    assert_eq!(text(&app), "a\na\nb\nb");
    assert_eq!(selections(&app), [(2, 2), (6, 6)]);
}

#[test]
fn clone_on_a_lone_cr_file_duplicates_the_cr_verbatim() {
    let mut app = app_with("A\rB", 1);
    let id = app.active;
    clone_line_down(&mut app, id);
    assert_eq!(text(&app), "A\rB\nA\rB");
}

#[test]
fn move_line_up_swaps_with_the_previous_line_preserving_column() {
    let mut app = app_with("one\ntwo\nthree", "one\ntw".len());
    let id = app.active;
    move_line_up(&mut app, id);
    assert_eq!(text(&app), "two\none\nthree");
    assert_eq!(selections(&app), [(2, 2)]);
}

#[test]
fn move_line_up_at_the_first_line_is_a_no_op() {
    let mut app = app_with("one\ntwo", 1);
    let id = app.active;
    move_line_up(&mut app, id);
    assert_eq!(text(&app), "one\ntwo");
    assert_eq!(journal_len(&app), 0);
}

#[test]
fn move_line_down_at_the_last_line_is_a_no_op() {
    let mut app = app_with("one\ntwo", "one\n".len() + 1);
    let id = app.active;
    move_line_down(&mut app, id);
    assert_eq!(text(&app), "one\ntwo");
    assert_eq!(journal_len(&app), 0);
}

#[test]
fn a_selected_block_moves_up_with_its_selection() {
    let mut app = selected(FOUR_LINES, 2, 5);
    let id = app.active;
    move_line_up(&mut app, id);
    assert_eq!(text(&app), "b\nc\na\nd");
    assert_eq!(selections(&app), [(0, 3)]);
}

#[test]
fn a_selected_block_moves_down_with_its_selection() {
    let mut app = selected(FOUR_LINES, 2, 5);
    let id = app.active;
    move_line_down(&mut app, id);
    assert_eq!(text(&app), "a\nd\nb\nc");
    assert_eq!(selections(&app), [(4, 7)]);
}

#[test]
fn moving_a_reversed_selection_keeps_its_direction() {
    let mut app = selected(FOUR_LINES, 5, 2);
    let id = app.active;
    move_line_down(&mut app, id);
    assert_eq!(selections(&app), [(7, 4)]);
}

#[test]
fn a_selection_ending_at_column_zero_still_ends_at_column_zero_after_moving() {
    let mut down = selected(FOUR_LINES, 0, 4);
    let id = down.active;
    move_line_down(&mut down, id);
    assert_eq!(text(&down), "c\na\nb\nd");
    assert_eq!(selections(&down), [(2, 6)]);

    let mut up = selected(FOUR_LINES, 2, 6);
    let id = up.active;
    move_line_up(&mut up, id);
    assert_eq!(text(&up), "b\nc\na\nd");
    assert_eq!(selections(&up), [(0, 4)]);
}

#[test]
fn every_cursor_survives_a_move() {
    let mut app = app_with("a\nb\nc\nd\ne", 0);
    let id = app.active;
    cursors(&mut app, id, &[(2, 2), (6, 6)]);
    move_line_down(&mut app, id);
    assert_eq!(text(&app), "a\nc\nb\ne\nd");
    assert_eq!(selections(&app), [(4, 4), (8, 8)]);
}

#[test]
fn carets_on_adjacent_lines_move_as_one_block() {
    let mut app = app_with(FOUR_LINES, 0);
    let id = app.active;
    cursors(&mut app, id, &[(2, 2), (6, 6)]);
    move_line_up(&mut app, id);
    assert_eq!(text(&app), "b\na\nd\nc");
    assert_eq!(selections(&app), [(0, 0), (4, 4)]);
}

#[test]
fn a_block_at_the_top_stays_while_another_block_moves_up() {
    let mut app = app_with("a\nb\nc", 0);
    let id = app.active;
    cursors(&mut app, id, &[(0, 0), (4, 4)]);
    move_line_up(&mut app, id);
    assert_eq!(text(&app), "a\nc\nb");
    assert_eq!(selections(&app), [(0, 0), (2, 2)]);
}

#[test]
fn the_empty_line_after_a_trailing_newline_is_an_ordinary_row_to_move_down_across() {
    let mut app = selected("a\nb\n", 0, 4);
    let id = app.active;
    move_line_down(&mut app, id);
    assert_eq!(text(&app), "\na\nb");
    assert_eq!(selections(&app), [(1, 4)]);
}

#[test]
fn a_caret_on_the_last_real_line_swaps_it_with_the_trailing_empty_line() {
    let mut app = app_with("a\nb\n", 0);
    let id = app.active;
    cursors(&mut app, id, &[(2, 2)]);
    move_line_down(&mut app, id);
    assert_eq!(text(&app), "a\n\nb");
    assert_eq!(selections(&app), [(3, 3)]);
}

#[test]
fn a_block_that_includes_the_last_row_cannot_move_down() {
    let mut app = selected("a\nb", 0, 3);
    let id = app.active;
    move_line_down(&mut app, id);
    assert_eq!(text(&app), "a\nb");
    assert_eq!(selections(&app), [(0, 3)]);
    assert_eq!(journal_len(&app), 0);
}

#[test]
fn moving_the_unterminated_last_line_up_swaps_it_in_with_its_selection() {
    let mut app = selected("a\nb", 2, 3);
    let id = app.active;
    move_line_up(&mut app, id);
    assert_eq!(text(&app), "b\na");
    assert_eq!(selections(&app), [(0, 1)]);
}

#[test]
fn swapping_identical_lines_follows_the_caret_without_journaling_or_dirtying() {
    let mut app = app_with("a\na\nb", 2);
    let id = app.active;
    move_line_up(&mut app, id);
    assert_eq!(text(&app), "a\na\nb");
    assert_eq!(selections(&app), [(0, 0)]);
    assert_eq!(journal_len(&app), 0);
    assert!(!app.doc(id).unwrap().is_dirty());
    move_line_down(&mut app, id);
    assert_eq!(selections(&app), [(2, 2)]);
    assert_eq!(journal_len(&app), 0);
}

#[test]
fn touching_selections_merge_when_a_move_brings_them_together() {
    let mut app = app_with(FOUR_LINES, 0);
    let id = app.active;
    cursors(&mut app, id, &[(0, 2), (4, 4)]);
    move_line_up(&mut app, id);
    assert_eq!(text(&app), "a\nc\nb\nd");
    assert_eq!(selections(&app), [(0, 2)]);
}

#[test]
fn moving_the_last_line_of_a_crlf_file_keeps_its_terminator() {
    let mut app = app_with("A\r\nB", "A\r\n".len());
    let id = app.active;
    move_line_up(&mut app, id);
    assert_eq!(text(&app), "B\r\nA");
}

#[test]
fn moving_a_line_below_the_crlf_files_last_unterminated_line_keeps_it_unterminated() {
    let mut app = app_with("A\r\nB", 0);
    let id = app.active;
    move_line_down(&mut app, id);
    assert_eq!(text(&app), "B\r\nA");
}

#[test]
fn move_line_up_preserves_crlf_terminators_between_two_crlf_lines() {
    let mut app = app_with("one\r\ntwo\r\nthree", "one\r\ntw".len());
    let id = app.active;
    move_line_up(&mut app, id);
    assert_eq!(text(&app), "two\r\none\r\nthree");
}

#[test]
fn a_lone_cr_file_has_a_single_line_so_move_up_and_down_are_both_no_ops() {
    let mut app = app_with("A\rB", 1);
    let id = app.active;
    assert_eq!(app.doc(id).unwrap().buffer().line_count(), 1);
    move_line_up(&mut app, id);
    move_line_down(&mut app, id);
    assert_eq!(text(&app), "A\rB");
}

#[test]
fn each_line_command_undoes_in_one_step_restoring_text_and_selection() {
    type Command = fn(&mut App, DocumentId);
    let commands: [(&str, Command); 4] = [
        ("clone-up", clone_line_up),
        ("clone-down", clone_line_down),
        ("move-up", move_line_up),
        ("move-down", move_line_down),
    ];
    for (name, command) in commands {
        let mut app = selected(FOUR_LINES, 2, 5);
        let id = app.active;
        command(&mut app, id);
        assert_ne!(text(&app), FOUR_LINES, "{name} must edit");
        assert_eq!(journal_len(&app), 1, "{name} is one journal step");
        undo(&mut app, id);
        assert_eq!(text(&app), FOUR_LINES, "{name}");
        assert_eq!(selections(&app), [(2, 5)], "{name}");
    }
}

#[test]
fn read_only_blocks_line_commands() {
    let mut app = app_with("one\ntwo\nthree", "one\n".len());
    let id = app.active;
    app.doc_mut(id).unwrap().read_only = crate::document::ReadOnly::Always;
    let before = text(&app);

    indent(&mut app, id);
    assert_eq!(text(&app), before, "indent");
    outdent(&mut app, id);
    assert_eq!(text(&app), before, "outdent");
    delete_line(&mut app, id);
    assert_eq!(text(&app), before, "delete-line");
    clone_line_up(&mut app, id);
    assert_eq!(text(&app), before, "clone-up");
    clone_line_down(&mut app, id);
    assert_eq!(text(&app), before, "clone-down");
    move_line_up(&mut app, id);
    assert_eq!(text(&app), before, "move-up");
    move_line_down(&mut app, id);
    assert_eq!(text(&app), before, "move-down");
    assert_eq!(journal_len(&app), 0);
}

#[test]
fn moving_a_line_that_would_fuse_a_lone_cr_into_crlf_is_refused_with_a_warning() {
    for (name, run) in [
        ("down", move_line_down as fn(&mut App, DocumentId)),
        ("up", move_line_up),
    ] {
        let at = if name == "down" { 0 } else { 2 };
        let mut app = app_with("a\nb\r", at);
        let id = app.active;
        run(&mut app, id);
        assert_eq!(text(&app), "a\nb\r", "{name}");
        assert_eq!(journal_len(&app), 0, "{name}");
        assert!(
            crate::messages::newest_text(&app).is_some_and(|m| m.contains("lone carriage return")),
            "{name}: the refusal tells the user"
        );
    }
}

#[test]
fn cloning_an_unterminated_line_ending_in_a_lone_cr_is_refused_with_a_warning() {
    for (name, run) in [
        ("down", clone_line_down as fn(&mut App, DocumentId)),
        ("up", clone_line_up),
    ] {
        let mut app = app_with("a\nb\r", 2);
        let id = app.active;
        run(&mut app, id);
        assert_eq!(text(&app), "a\nb\r", "{name}");
        assert_eq!(journal_len(&app), 0, "{name}");
        assert!(
            crate::messages::newest_text(&app).is_some_and(|m| m.contains("lone carriage return")),
            "{name}: the refusal tells the user"
        );
    }
}

#[test]
fn a_lone_cr_that_stays_before_a_crlf_terminator_moves_freely() {
    let mut app = app_with("a\r\nb\r\r\nc", 0);
    let id = app.active;
    move_line_down(&mut app, id);
    assert_eq!(text(&app), "b\r\r\na\r\nc");
}
