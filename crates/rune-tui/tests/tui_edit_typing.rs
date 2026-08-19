#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod tui_edit_common;

use rune_tui::app::{self};
use rune_tui::keymap::{KeyCode, Mods};
use rune_tui::runtime::{Effects, Msg};

use tui_edit_common::{SHIFT, SUP, SUP_SHIFT, app_for, press};

#[test]
fn typing_inserts_characters_in_order_and_moves_the_caret() {
    let mut app = app_for("", 0);
    for ch in "hi!".chars() {
        press(&mut app, KeyCode::Char(ch), Mods::NONE);
    }
    assert_eq!(app.active_doc_mut().buffer.content(), "hi!");
    assert_eq!(app.active_doc_mut().cursors.primary().position, 3);
}

#[test]
fn typing_marks_the_buffer_dirty() {
    let mut app = app_for("hi", 0);
    assert!(!app.is_dirty());
    press(&mut app, KeyCode::Char('!'), Mods::NONE);
    assert!(app.is_dirty());
}

#[test]
fn typing_over_a_selection_replaces_it() {
    let mut app = app_for("hello world", 0);
    for _ in 0..5 {
        press(&mut app, KeyCode::Right, SHIFT);
    }
    assert!(app.active_doc_mut().cursors.primary().has_selection());

    press(&mut app, KeyCode::Char('X'), Mods::NONE);
    assert_eq!(app.active_doc_mut().buffer.content(), "X world");
    let c = app.active_doc_mut().cursors.primary();
    assert_eq!(c.position, 1);
    assert!(!c.has_selection());
}

#[test]
fn backspace_key_removes_the_char_to_the_left() {
    let mut app = app_for("abc", 1);
    press(&mut app, KeyCode::Backspace, Mods::NONE);
    assert_eq!(app.active_doc_mut().buffer.content(), "bc");
    assert_eq!(app.active_doc_mut().cursors.primary().position, 0);
}

#[test]
fn delete_key_removes_the_char_to_the_right() {
    let mut app = app_for("abc", 0);
    press(&mut app, KeyCode::Delete, Mods::NONE);
    assert_eq!(app.active_doc_mut().buffer.content(), "bc");
    assert_eq!(app.active_doc_mut().cursors.primary().position, 0);
}

#[test]
fn enter_inserts_a_newline_preserving_indentation() {
    let mut app = app_for("  indented", 10);
    press(&mut app, KeyCode::Enter, Mods::NONE);
    assert_eq!(app.active_doc_mut().buffer.content(), "  indented\n  ");
}

#[test]
fn tab_indents_the_current_line_shift_tab_outdents_it() {
    let mut app = app_for("hello", 2);
    press(&mut app, KeyCode::Tab, Mods::NONE);
    assert_eq!(app.active_doc_mut().buffer.content(), "\thello");

    press(&mut app, KeyCode::Tab, SHIFT);
    assert_eq!(app.active_doc_mut().buffer.content(), "hello");
}

#[test]
fn a_termina_backtab_event_dedents_the_current_line() {
    use termina::event::{KeyCode as TK, KeyEvent, KeyEventKind, KeyEventState, Modifiers as TM};

    let mut app = app_for("\thello", 1);
    let input = rune_tui::keymap::from_termina(KeyEvent {
        code: TK::BackTab,
        modifiers: TM::SHIFT,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    })
    .unwrap();

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(input), &mut effects);
    app.sync_view();

    assert_eq!(app.active_doc_mut().buffer.content(), "hello");
}

#[test]
fn tab_indents_every_line_of_a_shift_selected_block_and_keeps_the_selection() {
    let mut app = app_for("one\ntwo\nthree", 0);
    press(&mut app, KeyCode::Down, SHIFT);
    press(&mut app, KeyCode::Down, SHIFT);
    press(&mut app, KeyCode::End, SHIFT);
    press(&mut app, KeyCode::Tab, Mods::NONE);
    assert_eq!(
        app.active_doc_mut().buffer.content(),
        "\tone\n\ttwo\n\tthree"
    );
    assert!(app.active_doc_mut().cursors.primary().has_selection());

    press(&mut app, KeyCode::Tab, SHIFT);
    assert_eq!(app.active_doc_mut().buffer.content(), "one\ntwo\nthree");
}

#[test]
fn undo_restores_byte_exact_content_and_redo_reapplies_it() {
    let mut app = app_for("hello", 5);
    press(&mut app, KeyCode::Char('!'), Mods::NONE);
    assert_eq!(app.active_doc_mut().buffer.content(), "hello!");

    press(&mut app, KeyCode::Char('z'), SUP);
    assert_eq!(app.active_doc_mut().buffer.content(), "hello");

    press(&mut app, KeyCode::Char('z'), SUP_SHIFT);
    assert_eq!(app.active_doc_mut().buffer.content(), "hello!");
}

#[test]
fn undo_redo_never_split_a_cjk_or_emoji_char() {
    let mut app = app_for("你好", "你".len());
    let original = app.active_doc_mut().buffer.content().to_string();

    press(&mut app, KeyCode::Char('\u{1f389}'), Mods::NONE);
    assert_eq!(app.active_doc_mut().buffer.content(), "你\u{1f389}好");

    press(&mut app, KeyCode::Char('z'), SUP);
    assert_eq!(
        app.active_doc_mut().buffer.content(),
        original,
        "undo must restore the original content byte-exact, including CJK"
    );

    press(&mut app, KeyCode::Char('z'), SUP_SHIFT);
    assert_eq!(app.active_doc_mut().buffer.content(), "你\u{1f389}好");
}

#[test]
fn undo_redo_restore_the_recorded_cursor_position() {
    let mut app = app_for("hello", 2);
    press(&mut app, KeyCode::Char('X'), Mods::NONE);
    assert_eq!(app.active_doc_mut().cursors.primary().position, 3);

    press(&mut app, KeyCode::Char('z'), SUP);
    assert_eq!(app.active_doc_mut().buffer.content(), "hello");
    assert_eq!(
        app.active_doc_mut().cursors.primary().position,
        2,
        "undo must restore the pre-edit cursor position"
    );

    press(&mut app, KeyCode::Char('z'), SUP_SHIFT);
    assert_eq!(
        app.active_doc_mut().cursors.primary().position,
        3,
        "redo must restore the post-edit cursor position"
    );
}

#[test]
fn undo_with_an_empty_journal_is_a_no_op() {
    let mut app = app_for("hello", 0);
    press(&mut app, KeyCode::Char('z'), SUP);
    assert_eq!(app.active_doc_mut().buffer.content(), "hello");
}

#[test]
fn moving_down_onto_a_heading_lands_before_its_marker() {
    let content = "plain\n# Heading\ntext\n";
    let mut app = app_for(content, 0);
    press(&mut app, KeyCode::Down, Mods::NONE);
    assert_eq!(
        app.active_doc_mut().cursors.primary().position,
        "plain\n".len()
    );
}

#[test]
fn down_then_up_over_a_concealing_heading_lands_where_a_fresh_caret_would() {
    let content = "# Heading\ntext\n";

    let mut baseline = app_for(content, 0);
    press(&mut baseline, KeyCode::Char('X'), Mods::NONE);
    let baseline_content = baseline.active_doc_mut().buffer.content().to_string();

    let mut roundtrip = app_for(content, 0);
    press(&mut roundtrip, KeyCode::Down, Mods::NONE);
    press(&mut roundtrip, KeyCode::Up, Mods::NONE);
    assert_eq!(
        roundtrip.active_doc_mut().cursors.primary().position,
        0,
        "the round trip must resettle the caret back to byte 0, not inside the revealed text"
    );
    press(&mut roundtrip, KeyCode::Char('X'), Mods::NONE);
    let roundtrip_content = roundtrip.active_doc_mut().buffer.content().to_string();

    assert_eq!(
        roundtrip_content, baseline_content,
        "a Down/Up round trip over a concealing heading must not change where typing lands"
    );
}
