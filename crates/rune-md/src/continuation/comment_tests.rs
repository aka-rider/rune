use super::*;

const HASH: &[&str] = &["#"];
const RUST: &[&str] = &["///", "//!", "//"];

fn insert(from: usize, to: usize, continuation: &str) -> EnterPlan {
    EnterPlan::Insert {
        replace: from..to,
        continuation: continuation.to_string(),
    }
}

fn replace(range: std::ops::Range<usize>) -> EnterPlan {
    EnterPlan::Replace {
        range,
        text: String::new(),
    }
}

fn at_end(line: &str, tokens: &[&str]) -> EnterPlan {
    enter_comment(line, line.len(), tokens, false)
}

#[test]
fn comment_line_continues_with_token_and_space() {
    assert_eq!(at_end("# foo", HASH), insert(5, 5, "# "));
}

#[test]
fn indentation_before_the_token_is_kept() {
    assert_eq!(at_end("    # foo", HASH), insert(9, 9, "    # "));
    assert_eq!(at_end("\t# foo", HASH), insert(6, 6, "\t# "));
}

#[test]
fn longest_token_wins() {
    assert_eq!(at_end("/// doc", RUST), insert(7, 7, "/// "));
    assert_eq!(at_end("//! doc", RUST), insert(7, 7, "//! "));
    assert_eq!(at_end("// note", RUST), insert(7, 7, "// "));
}

#[test]
fn token_order_in_the_list_does_not_matter() {
    let reversed = &["//", "//!", "///"];
    assert_eq!(at_end("/// doc", reversed), insert(7, 7, "/// "));
}

#[test]
fn other_token_families_work() {
    assert_eq!(at_end("-- sql", &["--"]), insert(6, 6, "-- "));
    assert_eq!(at_end("% erl", &["%"]), insert(5, 5, "% "));
    assert_eq!(at_end("# php", &["//", "#"]), insert(5, 5, "# "));
}

#[test]
fn token_without_following_space_gains_one() {
    assert_eq!(at_end("#foo", HASH), insert(4, 4, "# "));
    assert_eq!(at_end("//foo", RUST), insert(5, 5, "// "));
}

#[test]
fn caret_must_be_at_or_after_the_token_end() {
    assert_eq!(enter_comment("# foo", 0, HASH, false), EnterPlan::Plain);
    assert_eq!(enter_comment("// foo", 1, RUST, false), EnterPlan::Plain);
    assert_eq!(enter_comment("    # foo", 3, HASH, false), EnterPlan::Plain);
    assert_eq!(enter_comment("# foo", 1, HASH, false), insert(1, 2, "# "));
    assert_eq!(enter_comment("# foo", 2, HASH, false), insert(1, 2, "# "));
}

#[test]
fn caret_past_the_end_is_clamped() {
    assert_eq!(enter_comment("# foo", 99, HASH, false), insert(5, 5, "# "));
}

#[test]
fn trailing_whitespace_before_the_caret_is_trimmed() {
    assert_eq!(at_end("# foo   ", HASH), insert(5, 8, "# "));
    assert_eq!(
        enter_comment("# foo  bar", 7, HASH, false),
        insert(5, 7, "# ")
    );
}

#[test]
fn trimming_stops_at_the_token() {
    assert_eq!(enter_comment("#   foo", 3, HASH, false), insert(1, 4, "# "));
}

#[test]
fn empty_comment_line_loses_its_token() {
    assert_eq!(at_end("# ", HASH), replace(0..2));
    assert_eq!(at_end("#", HASH), replace(0..1));
    assert_eq!(at_end("    // ", RUST), replace(4..7));
    assert_eq!(at_end("\t///\t", RUST), replace(1..5));
}

#[test]
fn shebang_on_the_first_line_is_not_a_comment() {
    let shebang = "#!/usr/bin/env python";
    assert_eq!(
        enter_comment(shebang, shebang.len(), HASH, true),
        EnterPlan::Plain
    );
}

#[test]
fn shebang_look_alike_on_a_later_line_is_a_comment() {
    let line = "#!/usr/bin/env python";
    assert_eq!(
        enter_comment(line, line.len(), HASH, false),
        insert(line.len(), line.len(), "# ")
    );
}

#[test]
fn first_line_comment_that_is_not_a_shebang_continues() {
    assert_eq!(enter_comment("# foo", 5, HASH, true), insert(5, 5, "# "));
    assert_eq!(
        enter_comment("//! foo", 7, RUST, true),
        insert(7, 7, "//! ")
    );
}

#[test]
fn lines_that_do_not_start_with_a_token_are_plain() {
    assert_eq!(at_end("x = 1", HASH), EnterPlan::Plain);
    assert_eq!(at_end("x = 1  # trailing", HASH), EnterPlan::Plain);
    assert_eq!(at_end("", HASH), EnterPlan::Plain);
    assert_eq!(at_end("   ", HASH), EnterPlan::Plain);
    assert_eq!(at_end("/ / x", RUST), EnterPlan::Plain);
}

#[test]
fn language_without_tokens_is_plain() {
    assert_eq!(at_end("# foo", &[]), EnterPlan::Plain);
}

#[test]
fn empty_token_never_matches() {
    assert_eq!(at_end("foo", &[""]), EnterPlan::Plain);
    assert_eq!(at_end("# foo", &["", "#"]), insert(5, 5, "# "));
}

#[test]
fn caret_inside_a_multibyte_char_is_plain() {
    assert_eq!(enter_comment("# é", 3, HASH, false), EnterPlan::Plain);
    assert_eq!(enter_comment("# é", 4, HASH, false), insert(4, 4, "# "));
}

#[test]
fn multibyte_text_around_the_caret_keeps_offsets_exact() {
    let line = "# héllo  wörld";
    assert_eq!(enter_comment(line, 9, HASH, false), insert(8, 10, "# "));
    assert_eq!(at_end("# 日本 ", HASH), insert(8, 9, "# "));
}

#[test]
fn carriage_return_ends_the_line_content() {
    assert_eq!(enter_comment("# foo\r", 5, HASH, false), insert(5, 5, "# "));
    assert_eq!(enter_comment("# foo\r", 6, HASH, false), insert(5, 5, "# "));
    assert_eq!(enter_comment("# \r", 2, HASH, false), replace(0..2));
}
