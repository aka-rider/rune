use super::*;

const HASH: &[&str] = &["#"];
const RUST: &[&str] = &["///", "//!", "//"];

fn in_list(line: &str) -> Option<Range<usize>> {
    backspace(line, line.len(), None)
}

fn in_comment(line: &str, tokens: &[&str]) -> Option<Range<usize>> {
    backspace(line, line.len(), Some(tokens))
}

#[test]
fn bare_marker_is_removed_whole() {
    assert_eq!(in_list("- "), Some(0..2));
    assert_eq!(in_list("1. "), Some(0..3));
    assert_eq!(in_list("1) "), Some(0..3));
    assert_eq!(in_list("- [ ] "), Some(0..6));
    assert_eq!(in_list("- [x] "), Some(0..6));
}

#[test]
fn marker_removal_takes_all_its_spacing() {
    assert_eq!(in_list("-   "), Some(0..4));
    assert_eq!(in_list("-\t"), Some(0..2));
}

#[test]
fn indentation_before_the_marker_stays() {
    assert_eq!(in_list("  - "), Some(2..4));
    assert_eq!(in_list("\t1. "), Some(1..4));
}

#[test]
fn quote_only_removes_the_last_bracket() {
    assert_eq!(in_list("> "), Some(0..2));
    assert_eq!(in_list(">"), Some(0..1));
    assert_eq!(in_list(">> "), Some(1..3));
    assert_eq!(in_list("> > "), Some(2..4));
}

#[test]
fn marker_inside_a_quote_goes_before_the_quote() {
    assert_eq!(in_list("> - "), Some(2..4));
    assert_eq!(in_list("> - [ ] "), Some(2..8));
}

#[test]
fn caret_must_be_at_the_end_of_the_line() {
    assert_eq!(backspace("- ", 1, None), None);
    assert_eq!(backspace("- ", 0, None), None);
    assert_eq!(backspace("# ", 1, Some(HASH)), None);
}

#[test]
fn caret_past_the_end_counts_as_the_end() {
    assert_eq!(backspace("- ", 99, None), Some(0..2));
}

#[test]
fn line_with_content_keeps_one_rune_delete() {
    assert_eq!(in_list("- foo"), None);
    assert_eq!(in_list("> foo"), None);
    assert_eq!(in_list("- [ ] x"), None);
    assert_eq!(in_comment("# foo", HASH), None);
}

#[test]
fn lines_without_a_prefix_keep_one_rune_delete() {
    for line in ["", "   ", "text", "-", "-foo", "---", "* * *", "1."] {
        assert_eq!(in_list(line), None, "{line:?}");
    }
}

#[test]
fn bare_comment_token_is_removed() {
    assert_eq!(in_comment("# ", HASH), Some(0..2));
    assert_eq!(in_comment("#", HASH), Some(0..1));
    assert_eq!(in_comment("    // ", RUST), Some(4..7));
    assert_eq!(in_comment("/// ", RUST), Some(0..4));
}

#[test]
fn comment_mode_ignores_list_markers() {
    assert_eq!(in_comment("- ", HASH), None);
    assert_eq!(in_comment("> ", HASH), None);
}

#[test]
fn comment_mode_without_a_matching_token_is_none() {
    assert_eq!(in_comment("", HASH), None);
    assert_eq!(in_comment("  ", HASH), None);
    assert_eq!(in_comment("# ", &[]), None);
    assert_eq!(in_comment("# ", &[""]), None);
}

#[test]
fn shebang_line_is_not_a_bare_token() {
    assert_eq!(in_comment("#!", HASH), None);
}

#[test]
fn caret_inside_a_multibyte_char_is_none() {
    assert_eq!(backspace("é", 1, None), None);
    assert_eq!(backspace("# é", 3, Some(HASH)), None);
}

#[test]
fn carriage_return_ends_the_line_content() {
    assert_eq!(backspace("- \r", 2, None), Some(0..2));
    assert_eq!(backspace("- \r", 3, None), Some(0..2));
    assert_eq!(backspace("# \r", 2, Some(HASH)), Some(0..2));
}
