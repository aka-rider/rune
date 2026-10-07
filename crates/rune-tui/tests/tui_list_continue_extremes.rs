#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod tui_edit_common;

use tui_edit_common::{backspace, enter, marked, with_carets};

#[test]
fn enter_in_an_empty_document_inserts_a_bare_newline() {
    let mut app = with_carets("|");
    enter(&mut app);
    assert_eq!(marked(&mut app), "\n|");
}

#[test]
fn enter_with_the_caret_at_offset_zero_of_an_item_pushes_the_whole_item_down() {
    let mut app = with_carets("|- item");
    enter(&mut app);
    assert_eq!(marked(&mut app), "\n|- item");
}

#[test]
fn enter_after_multibyte_item_text_continues_the_item() {
    let mut app = with_carets("- héllo wörld 日本|");
    enter(&mut app);
    assert_eq!(marked(&mut app), "- héllo wörld 日本\n- |");
}

#[test]
fn enter_between_multibyte_characters_moves_the_tail_into_the_new_item() {
    let mut app = with_carets("- 日本|語 é");
    enter(&mut app);
    assert_eq!(marked(&mut app), "- 日本\n- |語 é");
}

#[test]
fn enter_on_a_tab_indented_item_keeps_the_tab_indent() {
    let mut app = with_carets("- a\n\t- b|");
    enter(&mut app);
    assert_eq!(marked(&mut app), "- a\n\t- b\n\t- |");
}

#[test]
fn enter_on_an_empty_tab_indented_item_outdents_to_its_parent() {
    let mut app = with_carets("1. a\n\t- |");
    enter(&mut app);
    assert_eq!(marked(&mut app), "1. a\n2. |");
}

#[test]
fn blanks_after_the_caret_do_not_double_the_space_after_the_new_marker() {
    let mut app = with_carets("- ab| cd");
    enter(&mut app);
    assert_eq!(marked(&mut app), "- ab\n- |cd");
}

#[test]
fn an_item_numbered_at_the_ordered_limit_gets_a_plain_newline() {
    let mut app = with_carets("999999999. x|");
    enter(&mut app);
    assert_eq!(marked(&mut app), "999999999. x\n|");
}

#[test]
fn renumbering_stops_instead_of_overflowing_at_the_ordered_limit() {
    let mut app = with_carets("999999998. a|\n999999999. b");
    enter(&mut app);
    assert_eq!(marked(&mut app), "999999998. a\n999999999. |\n999999999. b");
}

#[test]
fn an_unterminated_fence_opener_in_a_list_item_does_not_gain_a_bullet() {
    let mut app = with_carets("- ```python|");
    enter(&mut app);
    assert_eq!(marked(&mut app), "- ```python\n|");
}

#[test]
fn an_unterminated_fence_opener_in_a_quote_does_not_gain_a_quote_marker() {
    let mut app = with_carets("> ```python|");
    enter(&mut app);
    assert_eq!(marked(&mut app), "> ```python\n|");
}

#[test]
fn enter_in_a_quoted_python_fence_continues_the_comment_inside_the_quote() {
    let mut app = with_carets("> ```python\n> # note|\n> ```");
    enter(&mut app);
    assert_eq!(marked(&mut app), "> ```python\n> # note\n> # |\n> ```");
}

#[test]
fn enter_on_an_empty_comment_in_a_quoted_fence_exits_the_comment_but_keeps_the_quote() {
    let mut app = with_carets("> ```python\n> # |\n> ```");
    enter(&mut app);
    assert_eq!(marked(&mut app), "> ```python\n> |\n> ```");
}

#[test]
fn backspace_on_a_bare_comment_token_in_a_quoted_fence_keeps_the_quote() {
    let mut app = with_carets("> ```python\n> # |\n> ```");
    backspace(&mut app);
    assert_eq!(marked(&mut app), "> ```python\n> |\n> ```");
}

#[test]
fn enter_in_a_python_fence_inside_a_list_item_keeps_the_item_indent() {
    let mut app = with_carets("- ```python\n  # note|\n  ```");
    enter(&mut app);
    assert_eq!(marked(&mut app), "- ```python\n  # note\n  # |\n  ```");
}

#[test]
fn a_child_item_whose_parent_is_beyond_the_scan_cap_exits_the_list_instead_of_outdenting() {
    let filler = "  more\n".repeat(250);
    let mut app = with_carets(&format!("1. parent\n{filler}  - |"));
    enter(&mut app);
    assert_eq!(marked(&mut app), format!("1. parent\n{filler}|"));
}

#[test]
fn a_child_item_whose_parent_is_inside_the_scan_cap_outdents() {
    let filler = "  more\n".repeat(100);
    let mut app = with_carets(&format!("1. parent\n{filler}  - |"));
    enter(&mut app);
    assert_eq!(marked(&mut app), format!("1. parent\n{filler}2. |"));
}
