use super::*;
use crate::continuation::parse_prefix;

fn insert(from: usize, to: usize, continuation: &str) -> EnterPlan {
    EnterPlan::Insert {
        replace: from..to,
        continuation: continuation.to_string(),
    }
}

fn replace(range: std::ops::Range<usize>, text: &str) -> EnterPlan {
    EnterPlan::Replace {
        range,
        text: text.to_string(),
    }
}

fn at_end(line: &str) -> EnterPlan {
    enter_list(line, line.len(), None, None)
}

fn under(parent: &str, line: &str) -> EnterPlan {
    enter_list(line, line.len(), None, Some(&parse_prefix(parent)))
}

#[test]
fn item_with_text_continues_with_the_same_marker() {
    assert_eq!(at_end("- foo"), insert(5, 5, "- "));
}

#[test]
fn bullet_char_and_spacing_are_kept() {
    assert_eq!(at_end("* foo"), insert(5, 5, "* "));
    assert_eq!(at_end("+ foo"), insert(5, 5, "+ "));
    assert_eq!(at_end("-   foo"), insert(7, 7, "-   "));
    assert_eq!(at_end("\t- foo"), insert(6, 6, "\t- "));
    assert_eq!(at_end("   - foo"), insert(8, 8, "   - "));
}

#[test]
fn trailing_whitespace_before_the_caret_is_trimmed() {
    assert_eq!(at_end("- foo   "), insert(5, 8, "- "));
    assert_eq!(at_end("- foo\t "), insert(5, 7, "- "));
    assert_eq!(enter_list("- foo  bar", 7, None, None), insert(5, 7, "- "));
}

#[test]
fn text_after_the_caret_is_left_for_the_new_item() {
    assert_eq!(enter_list("- foobar", 5, None, None), insert(5, 5, "- "));
}

#[test]
fn trimming_never_eats_into_the_marker() {
    assert_eq!(enter_list("- foo", 2, None, None), insert(2, 2, "- "));
    assert_eq!(enter_list("-   foo", 4, None, None), insert(4, 4, "-   "));
}

#[test]
fn caret_inside_indent_quote_or_marker_gets_a_plain_newline() {
    assert_eq!(enter_list("- foo", 0, None, None), EnterPlan::Plain);
    assert_eq!(enter_list("- foo", 1, None, None), EnterPlan::Plain);
    assert_eq!(enter_list("  - foo", 1, None, None), EnterPlan::Plain);
    assert_eq!(enter_list("  - foo", 3, None, None), EnterPlan::Plain);
    assert_eq!(enter_list("> foo", 0, None, None), EnterPlan::Plain);
    assert_eq!(enter_list("> foo", 1, None, None), EnterPlan::Plain);
    assert_eq!(enter_list("10. x", 2, None, None), EnterPlan::Plain);
    assert_eq!(enter_list("- [ ] x", 4, None, None), EnterPlan::Plain);
}

#[test]
fn caret_past_the_end_is_clamped_to_the_end() {
    assert_eq!(enter_list("- foo", 99, None, None), insert(5, 5, "- "));
    assert_eq!(enter_list("- ", usize::MAX, None, None), replace(0..2, ""));
}

#[test]
fn caret_inside_a_multibyte_char_gets_a_plain_newline() {
    assert_eq!(enter_list("- é", 3, None, None), EnterPlan::Plain);
    assert_eq!(enter_list("- é", 4, None, None), insert(4, 4, "- "));
    assert_eq!(enter_list("> 日本語", 3, None, None), EnterPlan::Plain);
}

#[test]
fn multibyte_text_before_and_after_the_caret_keeps_offsets_exact() {
    let line = "- héllo  wörld";
    assert_eq!(enter_list(line, 9, None, None), insert(8, 10, "- "));
    assert_eq!(at_end(line), insert(line.len(), line.len(), "- "));
}

#[test]
fn carriage_return_ends_the_line_content() {
    assert_eq!(enter_list("- foo\r", 5, None, None), insert(5, 5, "- "));
    assert_eq!(enter_list("- foo\r", 6, None, None), insert(5, 5, "- "));
    assert_eq!(enter_list("- \r", 2, None, None), replace(0..2, ""));
}

#[test]
fn lines_without_a_prefix_get_a_plain_newline() {
    for line in [
        "",
        "   ",
        "# Heading",
        "text",
        "-foo",
        "1.foo",
        "  continuation",
    ] {
        assert_eq!(at_end(line), EnterPlan::Plain, "{line:?}");
    }
}

#[test]
fn thematic_breaks_get_a_plain_newline() {
    for line in ["---", "***", "___", "* * *", "- - -", "  ***  "] {
        assert_eq!(at_end(line), EnterPlan::Plain, "{line:?}");
    }
}

#[test]
fn thematic_break_inside_a_quote_only_continues_the_quote() {
    assert_eq!(at_end("> ---"), insert(5, 5, "> "));
}

#[test]
fn task_continues_unchecked_with_the_same_bullet() {
    assert_eq!(at_end("- [ ] x"), insert(7, 7, "- [ ] "));
    assert_eq!(at_end("- [x] x"), insert(7, 7, "- [ ] "));
    assert_eq!(at_end("- [X] x"), insert(7, 7, "- [ ] "));
    assert_eq!(at_end("* [x] x"), insert(7, 7, "* [ ] "));
}

#[test]
fn checkbox_without_trailing_space_continues_as_a_plain_bullet() {
    assert_eq!(at_end("- [x]"), insert(5, 5, "- "));
}

#[test]
fn empty_task_exits_like_an_empty_bullet() {
    assert_eq!(at_end("- [ ] "), replace(0..6, ""));
}

#[test]
fn ordered_item_continues_with_the_next_number() {
    assert_eq!(at_end("1. a"), insert(4, 4, "2. "));
    assert_eq!(at_end("0. a"), insert(4, 4, "1. "));
    assert_eq!(at_end("1) a"), insert(4, 4, "2) "));
    assert_eq!(at_end("9. a"), insert(4, 4, "10. "));
    assert_eq!(at_end("007. a"), insert(6, 6, "008. "));
    assert_eq!(at_end("99999999. a"), insert(11, 11, "100000000. "));
}

#[test]
fn nine_digit_limit_never_overflows() {
    assert_eq!(at_end("999999999. a"), EnterPlan::Plain);
    assert_eq!(at_end("1234567890. a"), EnterPlan::Plain);
    assert_eq!(at_end("4294967295. a"), EnterPlan::Plain);
}

#[test]
fn quote_with_text_continues_the_quote() {
    assert_eq!(at_end("> foo"), insert(5, 5, "> "));
    assert_eq!(at_end(">> foo"), insert(6, 6, ">> "));
    assert_eq!(at_end(">foo"), insert(4, 4, ">"));
    assert_eq!(at_end("> > foo"), insert(7, 7, "> > "));
}

#[test]
fn list_item_inside_a_quote_continues_both() {
    assert_eq!(at_end("> - [ ] x"), insert(9, 9, "> - [ ] "));
    assert_eq!(at_end("> 1. x"), insert(6, 6, "> 2. "));
}

#[test]
fn empty_top_level_item_deletes_its_marker() {
    assert_eq!(at_end("- "), replace(0..2, ""));
    assert_eq!(at_end("-   "), replace(0..4, ""));
    assert_eq!(at_end("1. "), replace(0..3, ""));
    assert_eq!(at_end("  - "), replace(0..4, ""));
    assert_eq!(at_end("\t* "), replace(0..3, ""));
}

#[test]
fn empty_top_level_item_in_a_quote_keeps_the_quote_prefix() {
    assert_eq!(at_end("> - "), replace(2..4, ""));
    assert_eq!(at_end(">> 1. "), replace(3..6, ""));
}

#[test]
fn caret_short_of_the_end_of_an_empty_item_is_a_plain_newline() {
    assert_eq!(enter_list("-   ", 1, None, None), EnterPlan::Plain);
    assert_eq!(enter_list("-   ", 2, None, None), EnterPlan::Plain);
}

#[test]
fn empty_nested_bullet_becomes_a_sibling_of_its_parent() {
    assert_eq!(under("- a", "  - "), replace(0..4, "- "));
    assert_eq!(under("* a", "\t- "), replace(0..3, "* "));
    assert_eq!(under("  - a", "      - "), replace(0..8, "  - "));
}

#[test]
fn empty_nested_item_under_an_ordered_parent_takes_the_next_number() {
    assert_eq!(under("3. a", "   - "), replace(0..5, "4. "));
    assert_eq!(under("3) a", "    1) "), replace(0..7, "4) "));
}

#[test]
fn empty_nested_task_becomes_an_unchecked_sibling_of_a_task_parent() {
    assert_eq!(under("- [x] a", "  - [ ] "), replace(0..8, "- [ ] "));
}

#[test]
fn empty_nested_item_in_a_quote_keeps_the_parents_quote_prefix() {
    assert_eq!(under("> - a", ">   - "), replace(0..6, "> - "));
}

#[test]
fn parent_that_does_not_enclose_is_ignored() {
    assert_eq!(under("- a", "- "), replace(0..2, ""));
    assert_eq!(under("  - a", "- "), replace(0..2, ""));
    assert_eq!(under("> - a", "  - "), replace(0..4, ""));
    assert_eq!(under("text", "  - "), replace(0..4, ""));
}

#[test]
fn ordered_parent_at_the_digit_limit_falls_back_to_leaving_the_list() {
    assert_eq!(under("999999999. a", "            - "), replace(0..14, ""));
}

#[test]
fn nested_item_with_text_continues_at_its_own_level() {
    assert_eq!(under("- a", "  - b"), insert(5, 5, "  - "));
}

#[test]
fn empty_quote_line_continues_once() {
    assert_eq!(at_end("> "), insert(1, 2, "> "));
    assert_eq!(at_end(">"), insert(1, 1, ">"));
    assert_eq!(at_end(">> "), insert(2, 3, ">> "));
}

#[test]
fn empty_quote_after_a_quote_with_text_continues() {
    assert_eq!(enter_list("> ", 2, Some("> foo"), None), insert(1, 2, "> "));
    assert_eq!(enter_list("> ", 2, Some(""), None), insert(1, 2, "> "));
}

#[test]
fn second_empty_quote_line_exits_deleting_both_prefixes() {
    assert_eq!(
        enter_list("> ", 2, Some(">"), None),
        EnterPlan::ExitQuote {
            previous: 0..1,
            current: 0..2
        }
    );
    assert_eq!(
        enter_list(">>", 2, Some(">> "), None),
        EnterPlan::ExitQuote {
            previous: 0..3,
            current: 0..2
        }
    );
    assert_eq!(
        enter_list("  >", 3, Some("  > "), None),
        EnterPlan::ExitQuote {
            previous: 2..4,
            current: 2..3
        }
    );
}

#[test]
fn double_empty_quote_exit_needs_the_same_depth_and_no_marker() {
    assert_eq!(enter_list("> ", 2, Some(">> "), None), insert(1, 2, "> "));
    assert_eq!(enter_list(">> ", 3, Some("> "), None), insert(2, 3, ">> "));
    assert_eq!(enter_list("> ", 2, Some("> - "), None), insert(1, 2, "> "));
}

#[test]
fn quote_exit_tolerates_carriage_returns() {
    assert_eq!(
        enter_list(">\r", 1, Some(">\r"), None),
        EnterPlan::ExitQuote {
            previous: 0..1,
            current: 0..1
        }
    );
}

#[test]
fn blanks_after_the_caret_are_swallowed_so_the_new_item_has_no_double_space() {
    assert_eq!(enter_list("- ab cd", 4, None, None), insert(4, 5, "- "));
    assert_eq!(enter_list("- ab \t cd", 4, None, None), insert(4, 7, "- "));
    assert_eq!(enter_list("- é  x", 4, None, None), insert(4, 6, "- "));
    assert_eq!(enter_list("- ab  ", 4, None, None), insert(4, 6, "- "));
}

#[test]
fn a_line_opening_a_code_fence_is_plain_even_before_the_fence_has_content() {
    assert_eq!(at_end("- ```python"), EnterPlan::Plain);
    assert_eq!(at_end("- ~~~"), EnterPlan::Plain);
    assert_eq!(at_end("> ```"), EnterPlan::Plain);
    assert_eq!(at_end("1. ```rust"), EnterPlan::Plain);
    assert_eq!(enter_list("- ```python", 4, None, None), EnterPlan::Plain);
}

#[test]
fn fewer_than_three_backticks_do_not_open_a_fence() {
    assert_eq!(at_end("- ``x"), insert(5, 5, "- "));
    assert_eq!(at_end("- ~~x"), insert(5, 5, "- "));
}
