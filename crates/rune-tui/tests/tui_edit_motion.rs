#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod tui_edit_common;

use rune_tui::keymap::{KeyCode, Mods};

use tui_edit_common::{ALT, SHIFT, SUP, app_for, press};

#[test]
fn char_right_then_left_returns_to_start() {
    let mut app = app_for("hello", 0);
    press(&mut app, KeyCode::Right, Mods::NONE);
    assert_eq!(app.active_doc_mut().cursors.primary().position, 1);
    press(&mut app, KeyCode::Left, Mods::NONE);
    assert_eq!(app.active_doc_mut().cursors.primary().position, 0);
}

#[test]
fn char_left_at_buffer_start_does_not_go_negative() {
    let mut app = app_for("hello", 0);
    press(&mut app, KeyCode::Left, Mods::NONE);
    assert_eq!(app.active_doc_mut().cursors.primary().position, 0);
}

#[test]
fn word_right_left_navigate_word_boundaries() {
    let mut app = app_for("hello world", 0);
    press(&mut app, KeyCode::Right, ALT);
    assert_eq!(app.active_doc_mut().cursors.primary().position, 5);
    press(&mut app, KeyCode::Left, ALT);
    assert_eq!(app.active_doc_mut().cursors.primary().position, 0);
}

#[test]
fn home_end_move_to_line_boundaries() {
    let mut app = app_for("hello\nworld", 8);
    press(&mut app, KeyCode::Home, Mods::NONE);
    assert_eq!(app.active_doc_mut().cursors.primary().position, 6);
    press(&mut app, KeyCode::End, Mods::NONE);
    assert_eq!(app.active_doc_mut().cursors.primary().position, 11);
}

#[test]
fn shift_right_extends_a_selection_plain_right_collapses_it() {
    let mut app = app_for("hello", 0);
    press(&mut app, KeyCode::Right, SHIFT);
    let c = app.active_doc_mut().cursors.primary();
    assert_eq!((c.anchor, c.position), (0, 1));
    assert!(c.has_selection());

    press(&mut app, KeyCode::Right, Mods::NONE);
    let c = app.active_doc_mut().cursors.primary();
    assert!(!c.has_selection(), "a plain move consumes the selection");
}

#[test]
fn select_all_selects_the_whole_buffer() {
    let mut app = app_for("hello world", 3);
    press(&mut app, KeyCode::Char('a'), SUP);
    let c = app.active_doc_mut().cursors.primary();
    assert_eq!((c.anchor, c.position), (0, 11));
}

#[test]
fn escape_collapses_a_selection_to_the_caret() {
    let mut app = app_for("hello", 0);
    press(&mut app, KeyCode::Right, SHIFT);
    assert!(app.active_doc_mut().cursors.primary().has_selection());
    press(&mut app, KeyCode::Escape, Mods::NONE);
    let c = app.active_doc_mut().cursors.primary();
    assert!(!c.has_selection());
    assert_eq!(c.position, 1);
}

#[test]
fn page_down_then_page_up_returns_to_the_original_row() {
    let mut lines = String::new();
    for i in 0..200 {
        lines.push_str(&format!("line{i}\n"));
    }
    let mut app = app_for(&lines, 0);
    let before = app.active_doc_mut().cursors.primary();

    press(&mut app, KeyCode::PageDown, Mods::NONE);
    let after_down = app.active_doc_mut().cursors.primary();
    assert_ne!(
        after_down.position, before.position,
        "page down must move the caret"
    );

    press(&mut app, KeyCode::PageUp, Mods::NONE);
    let after_up = app.active_doc_mut().cursors.primary();
    assert_eq!(
        after_up.position, before.position,
        "page up must return to the original row"
    );
}
