//! Unit tests for the line commands, kept in a sibling file so the
//! commands stay inside the 500-line budget.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use crate::commands::edit::undo;
use crate::commands::test_support::{cursors, selecting};
use rune_core::cursor::{CursorSet, CursorSpec};
use rune_vfs::Mem;
use std::sync::Arc;

fn app_with(content: &str, cursor_offset: usize) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    let id = app.active;
    app.doc_mut(id).unwrap().cursors = CursorSet::new(cursor_offset.min(content.len()));
    app.doc_mut(id).unwrap().viewport.set_size(80, 23);
    app
}

#[test]
fn indent_inserts_a_leading_tab() {
    let mut app = app_with("hello", 2);
    let id = app.active;
    indent(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "\thello");
}

#[test]
fn tab_indents_every_line_the_selection_touches() {
    let mut app = app_with("a\nb\nc", 0);
    let id = app.active;
    selecting(&mut app, id, 0, 5);
    indent(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "\ta\n\tb\n\tc");
}

#[test]
fn a_selection_ending_at_column_zero_does_not_indent_that_line() {
    let mut app = app_with("a\nb\nc", 0);
    let id = app.active;
    selecting(&mut app, id, 0, 4);
    indent(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "\ta\n\tb\nc");
}

#[test]
fn the_selection_still_covers_the_same_lines_after_indent() {
    let mut app = app_with("a\nb", 0);
    let id = app.active;
    selecting(&mut app, id, 0, 3);
    indent(&mut app, id);
    let primary = app.doc(id).unwrap().cursors.primary();
    assert_eq!((primary.anchor.get(), primary.position.get()), (0, 5));
}

#[test]
fn indent_keeps_the_caret_column_when_there_is_no_selection() {
    let mut app = app_with("hello", 2);
    let id = app.active;
    indent(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "\thello");
    assert_eq!(app.doc(id).unwrap().cursors.primary().position.get(), 3);
}

#[test]
fn outdent_dedents_every_selected_line() {
    let mut app = app_with("\ta\n\tb", 0);
    let id = app.active;
    selecting(&mut app, id, 0, 5);
    outdent(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "a\nb");
}

#[test]
fn outdent_puts_a_caret_inside_removed_indentation_at_the_line_start() {
    let mut app = app_with("\thello", 1);
    let id = app.active;
    outdent(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "hello");
    assert_eq!(app.doc(id).unwrap().cursors.primary().position.get(), 0);
}

#[test]
fn outdent_over_a_selection_with_no_indentation_is_a_no_op() {
    let mut app = app_with("a\nb", 0);
    let id = app.active;
    selecting(&mut app, id, 0, 3);
    outdent(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "a\nb");
    assert_eq!(app.doc(id).unwrap().journal.len(), 0);
    let primary = app.doc(id).unwrap().cursors.primary();
    assert_eq!((primary.anchor.get(), primary.position.get()), (0, 3));
}

#[test]
fn indenting_a_selection_is_one_undo_step() {
    let mut app = app_with("a\nb\nc", 0);
    let id = app.active;
    selecting(&mut app, id, 0, 5);
    let steps_before = app.doc(id).unwrap().journal.len();
    indent(&mut app, id);
    assert_eq!(app.doc(id).unwrap().journal.len(), steps_before + 1);
    undo(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "a\nb\nc");
}

#[test]
fn two_cursors_on_one_line_still_indent_it_once() {
    let mut app = app_with("ab", 0);
    let id = app.active;
    let doc = app.doc_mut(id).unwrap();
    doc.cursors = doc.cursors.clone().add(CursorSpec {
        position: BufferOffset(2),
        anchor: BufferOffset(2),
        desired_col: VisualCol(0),
    });
    indent(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "\tab");
}

#[test]
fn outdent_removes_one_leading_tab() {
    let mut app = app_with("\thello", 3);
    let id = app.active;
    outdent(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "hello");
}

#[test]
fn outdent_removes_up_to_four_leading_spaces() {
    let mut app = app_with("    hello", 5);
    let id = app.active;
    outdent(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "hello");
}

#[test]
fn outdent_on_a_line_with_no_indentation_is_a_no_op() {
    let mut app = app_with("hello", 0);
    let id = app.active;
    outdent(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "hello");
    assert_eq!(app.doc(id).unwrap().journal.len(), 0);
}

fn content(app: &App) -> String {
    app.doc(app.active).unwrap().buffer.content().to_string()
}

fn carets(app: &App) -> Vec<usize> {
    app.doc(app.active)
        .unwrap()
        .cursors
        .all()
        .iter()
        .map(|c| c.position.get())
        .collect()
}

#[test]
fn delete_line_removes_the_middle_line_and_its_own_newline() {
    let mut app = app_with("one\ntwo\nthree", "one\n".len());
    let id = app.active;
    delete_line(&mut app, id);
    assert_eq!(content(&app), "one\nthree");
}

#[test]
fn delete_line_removes_every_line_of_a_selection() {
    let mut app = app_with("a\nb\nc\nd", 0);
    let id = app.active;
    selecting(&mut app, id, 0, 5);
    delete_line(&mut app, id);
    assert_eq!(content(&app), "d");
    assert_eq!(carets(&app), vec![1]);
}

#[test]
fn a_selection_ending_at_column_zero_spares_that_line() {
    let mut app = app_with("a\nb\nc\nd", 0);
    let id = app.active;
    selecting(&mut app, id, 0, 4);
    delete_line(&mut app, id);
    assert_eq!(content(&app), "c\nd");
}

#[test]
fn a_reversed_selection_deletes_the_same_lines() {
    let mut app = app_with("a\nb\nc\nd", 0);
    let id = app.active;
    selecting(&mut app, id, 5, 0);
    delete_line(&mut app, id);
    assert_eq!(content(&app), "d");
}

#[test]
fn a_selection_reaching_the_last_line_absorbs_the_newline_above() {
    let mut app = app_with("a\nb\nc\nd", 0);
    let id = app.active;
    selecting(&mut app, id, 2, 7);
    delete_line(&mut app, id);
    assert_eq!(content(&app), "a");
    assert_eq!(carets(&app), vec![1]);
}

#[test]
fn delete_line_on_the_last_line_absorbs_the_previous_newline() {
    let mut app = app_with("one\ntwo", "one\n".len());
    let id = app.active;
    delete_line(&mut app, id);
    assert_eq!(content(&app), "one");
}

#[test]
fn deleting_the_last_line_of_a_crlf_file_leaves_no_lone_carriage_return() {
    let mut app = app_with("A\r\nB", 3);
    let id = app.active;
    delete_line(&mut app, id);
    assert_eq!(content(&app), "A");
}

#[test]
fn the_caret_never_lands_between_cr_and_lf() {
    let mut app = app_with("xxxx\r\nA\r\nC", 4);
    let id = app.active;
    delete_line(&mut app, id);
    assert_eq!(content(&app), "A\r\nC");
    assert_eq!(carets(&app), vec![1]);
}

#[test]
fn delete_line_on_a_single_line_buffer_clears_it() {
    let mut app = app_with("only", 2);
    let id = app.active;
    delete_line(&mut app, id);
    assert_eq!(content(&app), "");
}

#[test]
fn selecting_the_whole_buffer_and_deleting_lines_clears_it() {
    let mut app = app_with("a\nb", 0);
    let id = app.active;
    selecting(&mut app, id, 0, 3);
    delete_line(&mut app, id);
    assert_eq!(content(&app), "");
    assert_eq!(carets(&app), vec![0]);
}

#[test]
fn delete_line_on_an_empty_buffer_is_a_true_no_op_not_a_journaled_step() {
    let mut app = app_with("", 0);
    let id = app.active;
    delete_line(&mut app, id);
    assert_eq!(content(&app), "");
    assert_eq!(
        app.doc(id).unwrap().journal.len(),
        0,
        "an empty buffer has nothing to delete, so no Step and nothing to undo"
    );
}

#[test]
fn carets_on_adjacent_lines_delete_both_lines_in_one_step() {
    let mut app = app_with("a\nb\nc\nd", 0);
    let id = app.active;
    cursors(&mut app, id, &[(0, 0), (2, 2)]);
    delete_line(&mut app, id);
    assert_eq!(content(&app), "c\nd");
    assert_eq!(carets(&app), vec![0]);
    assert_eq!(app.doc(id).unwrap().journal.len(), 1);
}

#[test]
fn separate_blocks_each_delete_their_lines_including_a_last_line_block() {
    let mut app = app_with("a\nb\nc\nd\ne", 0);
    let id = app.active;
    cursors(&mut app, id, &[(0, 0), (4, 4), (8, 8)]);
    delete_line(&mut app, id);
    assert_eq!(content(&app), "b\nd");
    assert_eq!(carets(&app), vec![0, 2]);
}

#[test]
fn the_caret_column_is_clamped_to_a_shorter_landing_line() {
    let mut app = app_with("abcdef\nxy\nlonger", 4);
    let id = app.active;
    delete_line(&mut app, id);
    assert_eq!(content(&app), "xy\nlonger");
    assert_eq!(carets(&app), vec![2]);
}

#[test]
fn the_caret_lands_on_a_char_boundary_of_a_multibyte_landing_line() {
    let mut app = app_with("abcdef\n\u{e9}\u{e9}", 3);
    let id = app.active;
    delete_line(&mut app, id);
    assert_eq!(content(&app), "\u{e9}\u{e9}");
    let caret = carets(&app)[0];
    assert!(content(&app).is_char_boundary(caret));
}

#[test]
fn deleting_lines_is_one_undo_step_restoring_text_and_selection() {
    let mut app = app_with("a\nb\nc", 0);
    let id = app.active;
    selecting(&mut app, id, 0, 3);
    delete_line(&mut app, id);
    assert_eq!(content(&app), "c");
    assert_eq!(app.doc(id).unwrap().journal.len(), 1);
    undo(&mut app, id);
    assert_eq!(content(&app), "a\nb\nc");
    let primary = app.doc(id).unwrap().cursors.primary();
    assert_eq!((primary.anchor.get(), primary.position.get()), (0, 3));
}
