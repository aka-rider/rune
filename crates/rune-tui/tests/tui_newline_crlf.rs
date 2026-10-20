#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod tui_edit_common;

use rune_tui::app::App;
use rune_tui::keymap::KeyCode;

use tui_edit_common::{SHIFT, enter, press, with_carets, with_carets_at};

fn content(app: &mut App) -> String {
    app.active_doc_mut().buffer().content().to_string()
}

#[test]
fn enter_at_the_end_of_a_crlf_list_item_continues_it_with_a_crlf() {
    let mut app = with_carets("- a|\r\n");
    enter(&mut app);
    assert_eq!(content(&mut app), "- a\r\n- \r\n");
}

#[test]
fn enter_on_the_unterminated_last_line_of_a_crlf_document_uses_crlf() {
    let mut app = with_carets("x\r\n- b|");
    enter(&mut app);
    assert_eq!(content(&mut app), "x\r\n- b\r\n- ");
}

#[test]
fn shift_enter_in_a_crlf_document_inserts_a_crlf_and_the_indent() {
    let mut app = with_carets("  x|\r\ny");
    press(&mut app, KeyCode::Enter, SHIFT);
    assert_eq!(content(&mut app), "  x\r\n  \r\ny");
}

#[test]
fn enter_over_a_selection_in_a_crlf_document_replaces_it_with_a_crlf() {
    let mut app = with_carets("a|b\r\nc");
    press(&mut app, KeyCode::Right, SHIFT);
    enter(&mut app);
    assert_eq!(content(&mut app), "a\r\n\r\nc");
}

#[test]
fn renumbering_a_crlf_list_keeps_every_break_crlf() {
    let mut app = with_carets("1. a|\r\n2. b");
    enter(&mut app);
    assert_eq!(content(&mut app), "1. a\r\n2. \r\n3. b");
}

#[test]
fn comment_continuation_in_a_crlf_code_document_uses_crlf() {
    let mut app = with_carets_at("/x/main.py", "# note|\r\nx = 1\r\n");
    enter(&mut app);
    assert_eq!(content(&mut app), "# note\r\n# \r\nx = 1\r\n");
}

#[test]
fn a_bare_lf_document_keeps_getting_lf() {
    let mut app = with_carets("- a|\n");
    enter(&mut app);
    assert_eq!(content(&mut app), "- a\n- \n");
}
