use super::*;

fn marker_kind(line: &str) -> Option<MarkerKind> {
    parse_prefix(line).marker.map(|m| m.kind)
}

fn ordered(n: u32, delim: Delim, width: usize) -> Option<MarkerKind> {
    Some(MarkerKind::Ordered { n, delim, width })
}

#[test]
fn bullet_item_splits_into_indent_marker_and_content() {
    let prefix = parse_prefix("  - foo");
    let marker = prefix.marker.unwrap();
    assert_eq!(prefix.indent, 0..2);
    assert_eq!(marker.kind, MarkerKind::Bullet('-'));
    assert_eq!(marker.symbol, 2..3);
    assert_eq!(marker.gap, 3..4);
    assert_eq!(marker.span, 2..4);
    assert_eq!(marker.column, 2);
    assert_eq!(prefix.content_start, 4);
    assert_eq!(prefix.lead, "  - ");
    assert!(!prefix.empty);
}

#[test]
fn bullet_characters_are_dash_star_plus_only() {
    assert_eq!(marker_kind("- a"), Some(MarkerKind::Bullet('-')));
    assert_eq!(marker_kind("* a"), Some(MarkerKind::Bullet('*')));
    assert_eq!(marker_kind("+ a"), Some(MarkerKind::Bullet('+')));
    assert_eq!(marker_kind("• a"), None);
}

#[test]
fn marker_needs_spacing_after_the_symbol() {
    assert_eq!(marker_kind("-foo"), None);
    assert_eq!(marker_kind("-"), None);
    assert_eq!(marker_kind("1.foo"), None);
    assert_eq!(marker_kind("1."), None);
}

#[test]
fn tab_after_the_symbol_counts_as_spacing() {
    assert_eq!(marker_kind("-\tfoo"), Some(MarkerKind::Bullet('-')));
}

#[test]
fn checkbox_makes_a_task_in_all_three_spellings() {
    assert_eq!(marker_kind("- [ ] a"), Some(MarkerKind::Task('-')));
    assert_eq!(marker_kind("- [x] a"), Some(MarkerKind::Task('-')));
    assert_eq!(marker_kind("- [X] a"), Some(MarkerKind::Task('-')));
    assert_eq!(marker_kind("* [x] a"), Some(MarkerKind::Task('*')));
}

#[test]
fn checkbox_without_trailing_space_is_item_text() {
    let prefix = parse_prefix("- [x]");
    assert_eq!(prefix.marker.unwrap().kind, MarkerKind::Bullet('-'));
    assert_eq!(prefix.content_start, 2);
    assert_eq!(marker_kind("- [y] a"), Some(MarkerKind::Bullet('-')));
    assert_eq!(marker_kind("- [] a"), Some(MarkerKind::Bullet('-')));
}

#[test]
fn ordered_marker_accepts_one_to_nine_digits() {
    assert_eq!(marker_kind("1. a"), ordered(1, Delim::Dot, 1));
    assert_eq!(marker_kind("0. a"), ordered(0, Delim::Dot, 1));
    assert_eq!(marker_kind("3) a"), ordered(3, Delim::Paren, 1));
    assert_eq!(marker_kind("007. a"), ordered(7, Delim::Dot, 3));
    assert_eq!(
        marker_kind("999999999. a"),
        ordered(999_999_999, Delim::Dot, 9)
    );
}

#[test]
fn ten_digit_number_is_not_a_marker() {
    assert_eq!(marker_kind("1234567890. a"), None);
    assert_eq!(marker_kind("0000000001. a"), None);
}

#[test]
fn ordered_marker_rejects_other_delimiters_and_signs() {
    assert_eq!(marker_kind("1: a"), None);
    assert_eq!(marker_kind("-1. a"), None);
    assert_eq!(marker_kind("a. a"), None);
}

#[test]
fn thematic_breaks_are_not_markers() {
    for line in [
        "---",
        "***",
        "___",
        "* * *",
        "- - -",
        "  * * * *",
        "***  ",
        "_ _ _",
    ] {
        assert_eq!(marker_kind(line), None, "{line:?}");
    }
}

#[test]
fn near_thematic_breaks_stay_markers() {
    assert_eq!(marker_kind("* *"), Some(MarkerKind::Bullet('*')));
    assert_eq!(marker_kind("- - x"), Some(MarkerKind::Bullet('-')));
    assert_eq!(marker_kind("* * -"), Some(MarkerKind::Bullet('*')));
}

#[test]
fn quote_depth_counts_every_angle_bracket() {
    assert_eq!(parse_prefix("> a").quote.depth, 1);
    assert_eq!(parse_prefix(">> a").quote.depth, 2);
    assert_eq!(parse_prefix("> > a").quote.depth, 2);
    assert_eq!(parse_prefix(">  > a").quote.depth, 2);
    assert_eq!(parse_prefix("a > b").quote.depth, 0);
}

#[test]
fn quote_spans_cover_the_brackets_and_one_space_each() {
    let prefix = parse_prefix("  > > a");
    assert_eq!(prefix.quote.span, 2..6);
    assert_eq!(prefix.quote.last, 4..6);
    assert_eq!(prefix.content_start, 6);
    let tight = parse_prefix(">>a");
    assert_eq!(tight.quote.last, 1..2);
    assert_eq!(tight.content_start, 2);
}

#[test]
fn list_item_inside_quote_keeps_both_parts() {
    let prefix = parse_prefix("> - [ ] x");
    assert_eq!(prefix.quote.depth, 1);
    let marker = prefix.marker.unwrap();
    assert_eq!(marker.kind, MarkerKind::Task('-'));
    assert_eq!(marker.span, 2..8);
    assert_eq!(marker.column, 2);
    assert_eq!(prefix.content_start, 8);
}

#[test]
fn thematic_break_inside_quote_keeps_the_quote() {
    let prefix = parse_prefix("> ---");
    assert_eq!(prefix.quote.depth, 1);
    assert_eq!(prefix.marker, None);
}

#[test]
fn heading_and_plain_text_have_no_prefix() {
    for line in ["# Heading", "text", ""] {
        let prefix = parse_prefix(line);
        assert_eq!(prefix.marker, None);
        assert_eq!(prefix.quote.depth, 0);
        assert_eq!(prefix.content_start, 0);
    }
}

#[test]
fn line_of_only_a_marker_is_empty() {
    assert!(parse_prefix("- ").empty);
    assert!(parse_prefix("-   ").empty);
    assert!(parse_prefix("- [ ] ").empty);
    assert!(parse_prefix("1. ").empty);
    assert!(parse_prefix("> ").empty);
    assert!(parse_prefix(">").empty);
    assert!(parse_prefix("> - ").empty);
    assert!(parse_prefix("").empty);
}

#[test]
fn trailing_carriage_return_is_ignored() {
    let prefix = parse_prefix("- foo\r");
    assert_eq!(prefix.content_start, 2);
    assert!(parse_prefix("- \r").empty);
    assert!(parse_prefix("\r").empty);
}

#[test]
fn tab_advances_the_column_to_the_next_stop_of_four() {
    assert_eq!(parse_prefix("\t- a").marker.unwrap().column, 4);
    assert_eq!(parse_prefix("  \t- a").marker.unwrap().column, 4);
    assert_eq!(parse_prefix("    - a").marker.unwrap().column, 4);
    assert_eq!(parse_prefix("\t\t- a").marker.unwrap().column, 8);
}

#[test]
fn multibyte_content_does_not_disturb_offsets() {
    let prefix = parse_prefix("- héllo");
    assert_eq!(prefix.content_start, 2);
    let quoted = parse_prefix("> 日本語");
    assert_eq!(quoted.content_start, 2);
    assert_eq!(quoted.lead, "> ");
    let odd = parse_prefix("é- a");
    assert_eq!(odd.marker, None);
}

#[test]
fn content_column_measures_the_whole_prefix() {
    assert_eq!(parse_prefix("- a").content_column(), 2);
    assert_eq!(parse_prefix("    text").content_column(), 4);
    assert_eq!(parse_prefix("\ttext").content_column(), 4);
}

#[test]
fn continuation_of_a_bullet_keeps_char_and_spacing() {
    assert_eq!(parse_prefix("- a").continuation().unwrap(), "- ");
    assert_eq!(parse_prefix("  *   a").continuation().unwrap(), "  *   ");
    assert_eq!(parse_prefix("\t+\ta").continuation().unwrap(), "\t+\t");
}

#[test]
fn continuation_of_a_task_is_unchecked() {
    assert_eq!(parse_prefix("- [x] a").continuation().unwrap(), "- [ ] ");
    assert_eq!(parse_prefix("*  [X] a").continuation().unwrap(), "*  [ ] ");
}

#[test]
fn continuation_of_an_ordered_item_increments_and_keeps_width() {
    assert_eq!(parse_prefix("1. a").continuation().unwrap(), "2. ");
    assert_eq!(parse_prefix("9) a").continuation().unwrap(), "10) ");
    assert_eq!(parse_prefix("007. a").continuation().unwrap(), "008. ");
    assert_eq!(parse_prefix("0. a").continuation().unwrap(), "1. ");
}

#[test]
fn continuation_past_nine_digits_is_refused() {
    assert_eq!(parse_prefix("999999999. a").continuation(), None);
    assert_eq!(
        parse_prefix("999999998. a").continuation().unwrap(),
        "999999999. "
    );
}

#[test]
fn continuation_of_a_quoted_item_repeats_the_quote() {
    assert_eq!(parse_prefix("> 1. a").continuation().unwrap(), "> 2. ");
    assert_eq!(parse_prefix(">> a").continuation().unwrap(), ">> ");
}

#[test]
fn enclosing_needs_same_quote_depth_and_smaller_marker_column() {
    let outer = parse_prefix("- a");
    let inner = parse_prefix("  - b");
    assert!(outer.encloses(&inner));
    assert!(!inner.encloses(&outer));
    assert!(!outer.encloses(&parse_prefix("- c")));
    assert!(!outer.encloses(&parse_prefix(">   - c")));
    assert!(!parse_prefix("plain").encloses(&inner));
    assert!(!outer.encloses(&parse_prefix("  text")));
}
