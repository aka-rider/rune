use super::*;

fn shifted(item: &str, following: &[&str], inserted: u32) -> Vec<(usize, Range<usize>, String)> {
    renumber(&parse_prefix(item), following.iter().copied(), inserted)
        .into_iter()
        .map(|e| (e.line, e.range, e.text))
        .collect()
}

fn edit(line: usize, range: Range<usize>, text: &str) -> (usize, Range<usize>, String) {
    (line, range, text.to_string())
}

#[test]
fn following_siblings_shift_by_the_inserted_count() {
    assert_eq!(
        shifted("1. a", &["2. b", "3. c"], 1),
        vec![edit(0, 0..1, "3"), edit(1, 0..1, "4")]
    );
    assert_eq!(
        shifted("1. a", &["2. b", "3. c"], 2),
        vec![edit(0, 0..1, "4"), edit(1, 0..1, "5")]
    );
}

#[test]
fn run_stops_at_the_first_gap() {
    assert_eq!(
        shifted("1. a", &["2. b", "4. c", "5. d"], 1),
        vec![edit(0, 0..1, "3")]
    );
}

#[test]
fn first_sibling_must_follow_the_item_directly() {
    assert!(shifted("1. a", &["3. b"], 1).is_empty());
    assert!(shifted("1. a", &["1. b", "1. c"], 1).is_empty());
    assert!(shifted("5. a", &["5. b"], 1).is_empty());
}

#[test]
fn nothing_follows_nothing_changes() {
    assert!(shifted("1. a", &[], 1).is_empty());
}

#[test]
fn no_insertion_means_no_edit() {
    assert!(shifted("1. a", &["2. b"], 0).is_empty());
}

#[test]
fn item_without_an_ordered_marker_yields_nothing() {
    assert!(shifted("- a", &["2. b"], 1).is_empty());
    assert!(shifted("text", &["2. b"], 1).is_empty());
    assert!(shifted("- [ ] a", &["2. b"], 1).is_empty());
}

#[test]
fn run_stops_at_a_bullet_or_another_delimiter() {
    assert_eq!(
        shifted("1. a", &["2. b", "- x", "3. c"], 1),
        vec![edit(0, 0..1, "3")]
    );
    assert_eq!(
        shifted("1. a", &["2. b", "3) c"], 1),
        vec![edit(0, 0..1, "3")]
    );
    assert!(shifted("1) a", &["2. b"], 1).is_empty());
}

#[test]
fn run_stops_at_a_shallower_item() {
    assert_eq!(
        shifted("  1. a", &["  2. b", "3. c"], 1),
        vec![edit(0, 2..3, "3")]
    );
    assert!(shifted("  1. a", &["- x", "  2. b"], 1).is_empty());
}

#[test]
fn nested_items_belong_to_the_item_and_are_skipped() {
    assert_eq!(
        shifted("1. a", &["   1. n", "   2. m", "2. b", "3. c"], 1),
        vec![edit(2, 0..1, "3"), edit(3, 0..1, "4")]
    );
    assert_eq!(
        shifted("1. a", &["   - x", "2. b"], 1),
        vec![edit(1, 0..1, "3")]
    );
}

#[test]
fn indented_text_belongs_to_the_item_and_is_skipped() {
    assert_eq!(
        shifted("1. a", &["   more", "      deeper", "2. b"], 1),
        vec![edit(2, 0..1, "3")]
    );
}

#[test]
fn blank_lines_inside_a_loose_list_are_skipped() {
    assert_eq!(
        shifted("1. a", &["", "2. b", "", "3. c"], 1),
        vec![edit(1, 0..1, "3"), edit(3, 0..1, "4")]
    );
}

#[test]
fn unindented_text_ends_the_run() {
    assert!(shifted("1. a", &["text", "2. b"], 1).is_empty());
    assert!(shifted("1. a", &["", "text", "2. b"], 1).is_empty());
    assert!(shifted("1. a", &["# Heading", "2. b"], 1).is_empty());
    assert!(shifted("1. a", &["---", "2. b"], 1).is_empty());
}

#[test]
fn quote_depth_change_ends_the_run() {
    assert!(shifted("1. a", &["> 2. b"], 1).is_empty());
    assert!(shifted("> 1. a", &["2. b"], 1).is_empty());
    assert!(shifted("> 1. a", &["", "> 2. b"], 1).is_empty());
    assert_eq!(shifted("> 1. a", &["> 2. b"], 1), vec![edit(0, 2..3, "3")]);
}

#[test]
fn number_growing_a_digit_widens_the_edit_range_only_in_text() {
    assert_eq!(
        shifted("8. a", &["9. b", "10. c"], 1),
        vec![edit(0, 0..1, "10"), edit(1, 0..2, "11")]
    );
}

#[test]
fn leading_zeros_keep_their_width() {
    assert_eq!(
        shifted("007. a", &["008. b", "009. c"], 1),
        vec![edit(0, 0..3, "009"), edit(1, 0..3, "010")]
    );
}

#[test]
fn paren_delimiter_and_indentation_give_exact_ranges() {
    assert_eq!(shifted("\t1) a", &["\t2) b"], 1), vec![edit(0, 1..2, "3")]);
}

#[test]
fn zero_based_lists_renumber_too() {
    assert_eq!(
        shifted("0. a", &["1. b", "2. c"], 1),
        vec![edit(0, 0..1, "2"), edit(1, 0..1, "3")]
    );
}

#[test]
fn run_stops_before_a_number_that_would_pass_nine_digits() {
    assert_eq!(
        shifted("999999997. a", &["999999998. b", "999999999. c"], 1),
        vec![edit(0, 0..9, "999999999")]
    );
    assert!(shifted("999999998. a", &["999999999. b"], 2).is_empty());
}

#[test]
fn item_at_the_digit_limit_has_no_following_sibling_to_shift() {
    assert!(shifted("999999999. a", &["1000000000. b"], 1).is_empty());
}

#[test]
fn carriage_returns_are_tolerated() {
    assert_eq!(shifted("1. a\r", &["2. b\r"], 1), vec![edit(0, 0..1, "3")]);
}

#[test]
fn multibyte_content_does_not_disturb_ranges() {
    assert_eq!(shifted("1. é", &["2. 日本語"], 1), vec![edit(0, 0..1, "3")]);
}

#[test]
fn outdented_item_renumbers_the_parents_next_siblings() {
    let parent = parse_prefix("2. a");
    let after_outdent = ["   - nested sibling", "3. b", "4. c"];
    let edits = renumber(&parent, after_outdent.iter().copied(), 1);
    assert_eq!(
        edits
            .iter()
            .map(|e| (e.line, e.text.as_str()))
            .collect::<Vec<_>>(),
        vec![(1, "4"), (2, "5")]
    );
}

#[test]
fn edits_for_two_carets_in_one_run_add_their_shifts() {
    let lines = ["2. b", "3. c", "4. d", "5. e"];
    let first = renumber(&parse_prefix("1. a"), lines.iter().copied(), 1);
    let mut second = renumber(&parse_prefix("3. c"), lines.iter().skip(2).copied(), 1);
    for e in &mut second {
        e.line += 2;
    }
    let mut merged = first;
    for other in &second {
        if let Some(mine) = merged.iter_mut().find(|e| e.line == other.line) {
            mine.absorb(other);
        }
    }
    let texts: Vec<_> = merged.iter().map(|e| e.text.as_str()).collect();
    assert_eq!(texts, vec!["3", "4", "6", "7"]);
}

#[test]
fn absorbing_past_the_digit_limit_saturates_instead_of_overflowing() {
    let mut edits = renumber(&parse_prefix("999999997. a"), ["999999998. b"], 1);
    let twin = edits.clone();
    edits[0].absorb(&twin[0]);
    edits[0].absorb(&twin[0]);
    assert_eq!(edits[0].text, "999999999");
}
