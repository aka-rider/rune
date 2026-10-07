#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod tui_edit_common;

use rune_tui::commands::edit::newline;
use rune_tui::document::ReadOnly;
use rune_tui::keymap::KeyCode;

use tui_edit_common::{SHIFT, backspace, enter, marked, press, undo, with_carets, with_carets_at};

#[test]
fn enter_on_a_bullet_item_starts_the_next_bullet() {
    let mut app = with_carets("- one|");
    enter(&mut app);
    assert_eq!(marked(&mut app), "- one\n- |");
}

#[test]
fn enter_on_an_empty_bullet_exits_the_list() {
    let mut app = with_carets("- one\n- |");
    enter(&mut app);
    assert_eq!(marked(&mut app), "- one\n|");
}

#[test]
fn enter_keeps_the_bullet_char_indent_and_spacing() {
    let mut app = with_carets("  *   deep|");
    enter(&mut app);
    assert_eq!(marked(&mut app), "  *   deep\n  *   |");
}

#[test]
fn enter_in_the_middle_of_an_item_moves_the_tail_into_the_new_item() {
    let mut app = with_carets("- ab|cd");
    enter(&mut app);
    assert_eq!(marked(&mut app), "- ab\n- |cd");
}

#[test]
fn enter_trims_trailing_whitespace_before_the_caret() {
    let mut app = with_carets("- ab  |");
    enter(&mut app);
    assert_eq!(marked(&mut app), "- ab\n- |");
}

#[test]
fn enter_on_a_checked_task_continues_an_unchecked_one() {
    let mut app = with_carets("- [x] done|");
    enter(&mut app);
    assert_eq!(marked(&mut app), "- [x] done\n- [ ] |");
}

#[test]
fn enter_on_an_empty_nested_bullet_makes_it_a_sibling_of_its_parent() {
    let mut app = with_carets("- parent\n  - |");
    enter(&mut app);
    assert_eq!(marked(&mut app), "- parent\n- |");
}

#[test]
fn enter_on_an_empty_nested_item_under_an_ordered_parent_takes_the_next_number() {
    let mut app = with_carets("1. parent\n   - |");
    enter(&mut app);
    assert_eq!(marked(&mut app), "1. parent\n2. |");
}

#[test]
fn enter_on_a_quote_line_continues_the_quote() {
    let mut app = with_carets("> quoted|");
    enter(&mut app);
    assert_eq!(marked(&mut app), "> quoted\n> |");
}

#[test]
fn enter_on_a_task_inside_a_quote_keeps_the_quote_prefix() {
    let mut app = with_carets("> - [ ] x|");
    enter(&mut app);
    assert_eq!(marked(&mut app), "> - [ ] x\n> - [ ] |");
}

#[test]
fn a_single_empty_quote_line_continues_once_for_a_paragraph_break() {
    let mut app = with_carets("> a\n> |");
    enter(&mut app);
    assert_eq!(marked(&mut app), "> a\n>\n> |");
}

#[test]
fn a_second_empty_quote_line_exits_the_quote() {
    let mut app = with_carets("> a\n>\n> |");
    enter(&mut app);
    assert_eq!(marked(&mut app), "> a\n\n|");
}

#[test]
fn enter_on_an_indented_continuation_line_only_copies_the_indent() {
    let mut app = with_carets("- item\n  more text|");
    enter(&mut app);
    assert_eq!(marked(&mut app), "- item\n  more text\n  |");
}

#[test]
fn enter_inside_the_marker_is_a_plain_newline() {
    let mut app = with_carets("-| item");
    enter(&mut app);
    assert_eq!(marked(&mut app), "-\n| item");
}

#[test]
fn enter_on_a_thematic_break_is_a_plain_newline() {
    let mut app = with_carets("* * *|");
    enter(&mut app);
    assert_eq!(marked(&mut app), "* * *\n|");
}

#[test]
fn enter_after_a_heading_does_not_continue_the_hash() {
    let mut app = with_carets("# Heading|");
    enter(&mut app);
    assert_eq!(marked(&mut app), "# Heading\n|");
}

#[test]
fn a_ten_digit_number_is_not_a_marker() {
    let mut app = with_carets("1234567890. x|");
    enter(&mut app);
    assert_eq!(marked(&mut app), "1234567890. x\n|");
}

#[test]
fn enter_over_a_selection_replaces_it_with_a_plain_newline() {
    let mut app = with_carets("- a|b");
    press(&mut app, KeyCode::Right, SHIFT);
    enter(&mut app);
    assert_eq!(marked(&mut app), "- a\n|");
}

#[test]
fn shift_enter_on_a_list_item_is_a_plain_newline() {
    let mut app = with_carets("- item|");
    press(&mut app, KeyCode::Enter, SHIFT);
    assert_eq!(marked(&mut app), "- item\n|");
}

#[test]
fn enter_in_a_python_fence_continues_the_hash_comment() {
    let mut app = with_carets("```python\n# note|\n```");
    enter(&mut app);
    assert_eq!(marked(&mut app), "```python\n# note\n# |\n```");
}

#[test]
fn enter_on_an_empty_comment_in_a_fence_exits_the_comment() {
    let mut app = with_carets("```python\n# |\n```");
    enter(&mut app);
    assert_eq!(marked(&mut app), "```python\n|\n```");
}

#[test]
fn enter_in_a_fence_uses_the_fence_languages_own_comment_token() {
    let mut app = with_carets("```rust\n/// doc|\n```");
    enter(&mut app);
    assert_eq!(marked(&mut app), "```rust\n/// doc\n/// |\n```");
}

#[test]
fn enter_in_a_fence_without_comment_tokens_is_a_plain_newline() {
    let mut app = with_carets("```json\n  # x|\n```");
    enter(&mut app);
    assert_eq!(marked(&mut app), "```json\n  # x\n  |\n```");
}

#[test]
fn enter_in_an_unlabelled_fence_is_a_plain_newline() {
    let mut app = with_carets("```\n- item|\n```");
    enter(&mut app);
    assert_eq!(marked(&mut app), "```\n- item\n|\n```");
}

#[test]
fn enter_on_a_fence_line_is_a_plain_newline() {
    let mut app = with_carets("- ```python|\n  x = 1\n  ```");
    enter(&mut app);
    assert_eq!(marked(&mut app), "- ```python\n|\n  x = 1\n  ```");
}

#[test]
fn enter_on_a_bullet_just_outside_a_fence_still_continues_the_list() {
    let mut app = with_carets("```python\nx\n```\n- a|");
    enter(&mut app);
    assert_eq!(marked(&mut app), "```python\nx\n```\n- a\n- |");
}

#[test]
fn enter_in_yaml_frontmatter_continues_the_hash_comment() {
    let mut app = with_carets("---\n# note|\ntitle: x\n---\n");
    enter(&mut app);
    assert_eq!(marked(&mut app), "---\n# note\n# |\ntitle: x\n---\n");
}

#[test]
fn enter_in_a_python_document_continues_the_hash_comment() {
    let mut app = with_carets_at("/x/main.py", "# note|\nx = 1\n");
    enter(&mut app);
    assert_eq!(marked(&mut app), "# note\n# |\nx = 1\n");
}

#[test]
fn enter_in_a_python_document_does_not_continue_a_list_marker() {
    let mut app = with_carets_at("/x/main.py", "    - x|\n");
    enter(&mut app);
    assert_eq!(marked(&mut app), "    - x\n    |\n");
}

#[test]
fn enter_after_code_with_a_trailing_comment_is_a_plain_newline() {
    let mut app = with_carets_at("/x/main.py", "x = 1  # why|\n");
    enter(&mut app);
    assert_eq!(marked(&mut app), "x = 1  # why\n|\n");
}

#[test]
fn enter_on_a_shebang_on_the_first_line_is_a_plain_newline() {
    let mut app = with_carets_at("/x/run.py", "#!/usr/bin/env python|\n");
    enter(&mut app);
    assert_eq!(marked(&mut app), "#!/usr/bin/env python\n|\n");
}

#[test]
fn enter_in_a_rust_document_continues_the_longest_comment_token() {
    let mut app = with_carets_at("/x/lib.rs", "    //! crate doc|\n");
    enter(&mut app);
    assert_eq!(marked(&mut app), "    //! crate doc\n    //! |\n");
}

#[test]
fn enter_in_a_plain_text_document_copies_the_indent_only() {
    let mut app = with_carets_at("/x/notes.txt", "  - item|");
    enter(&mut app);
    assert_eq!(marked(&mut app), "  - item\n  |");
}

#[test]
fn backspace_on_a_bare_bullet_marker_removes_the_whole_marker() {
    let mut app = with_carets("- a\n- |");
    backspace(&mut app);
    assert_eq!(marked(&mut app), "- a\n|");
}

#[test]
fn backspace_on_a_bare_ordered_marker_removes_the_whole_marker() {
    let mut app = with_carets("1. a\n2. |");
    backspace(&mut app);
    assert_eq!(marked(&mut app), "1. a\n|");
}

#[test]
fn backspace_on_a_bare_task_marker_removes_the_checkbox_too() {
    let mut app = with_carets("- [ ] |");
    backspace(&mut app);
    assert_eq!(marked(&mut app), "|");
}

#[test]
fn backspace_on_a_bare_quote_removes_the_innermost_quote_marker() {
    let mut app = with_carets(">> |");
    backspace(&mut app);
    assert_eq!(marked(&mut app), ">|");
}

#[test]
fn backspace_after_item_text_deletes_one_character() {
    let mut app = with_carets("- ab|");
    backspace(&mut app);
    assert_eq!(marked(&mut app), "- a|");
}

#[test]
fn backspace_inside_the_marker_deletes_one_character() {
    let mut app = with_carets("- |a");
    backspace(&mut app);
    assert_eq!(marked(&mut app), "-|a");
}

#[test]
fn backspace_on_a_bare_comment_token_removes_the_token() {
    let mut app = with_carets_at("/x/main.py", "x = 1\n# |\n");
    backspace(&mut app);
    assert_eq!(marked(&mut app), "x = 1\n|\n");
}

#[test]
fn backspace_on_a_list_marker_inside_a_code_fence_deletes_one_character() {
    let mut app = with_carets("```python\n- |\n```");
    backspace(&mut app);
    assert_eq!(marked(&mut app), "```python\n-|\n```");
}

#[test]
fn backspace_on_a_bare_marker_is_one_undo_step() {
    let mut app = with_carets("- a\n- |");
    backspace(&mut app);
    undo(&mut app);
    assert_eq!(marked(&mut app), "- a\n- |");
}

#[test]
fn a_read_only_document_refuses_enter_and_stays_unchanged() {
    let mut app = with_carets("- item|");
    let id = app.active;
    app.doc_mut(id).unwrap().read_only = ReadOnly::Always;
    let posts_before = rune_tui::messages::posts(&app);

    newline(&mut app, id);

    assert_eq!(marked(&mut app), "- item|");
    assert!(rune_tui::messages::posts(&app) > posts_before);
    assert!(!app.is_dirty());
}
