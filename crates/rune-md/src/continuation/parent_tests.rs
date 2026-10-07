use super::*;

fn parent_of(child: &str, above: &[&str]) -> Option<Prefix> {
    find_parent(&parse_prefix(child), above.iter().copied()).map(|parent| parent.prefix)
}

fn lead_of(child: &str, above: &[&str]) -> Option<String> {
    parent_of(child, above).map(|p| p.lead)
}

#[test]
fn nearest_item_with_a_smaller_indent_is_the_parent() {
    assert_eq!(lead_of("  - ", &["- a"]).as_deref(), Some("- "));
    assert_eq!(
        lead_of("    - ", &["  - b", "- a"]).as_deref(),
        Some("  - ")
    );
}

#[test]
fn siblings_at_the_same_indent_are_skipped() {
    assert_eq!(
        lead_of("  - ", &["  - b", "  - c", "- a"]).as_deref(),
        Some("- ")
    );
}

#[test]
fn deeper_items_are_skipped() {
    assert_eq!(
        lead_of("  - ", &["    - deep", "  - b", "- a"]).as_deref(),
        Some("- ")
    );
}

#[test]
fn continuation_text_inside_the_items_is_skipped() {
    assert_eq!(
        lead_of("  - ", &["    more", "  text", "- a"]).as_deref(),
        Some("- ")
    );
}

#[test]
fn blank_lines_inside_a_loose_list_are_skipped() {
    assert_eq!(
        lead_of("  - ", &["", "  - b", "", "- a"]).as_deref(),
        Some("- ")
    );
}

#[test]
fn unindented_paragraph_ends_the_search() {
    assert_eq!(parent_of("  - ", &["text", "- a"]), None);
    assert_eq!(parent_of("  - ", &["# Heading", "- a"]), None);
}

#[test]
fn thematic_break_ends_the_search() {
    assert_eq!(parent_of("  - ", &["---", "- a"]), None);
}

#[test]
fn quote_depth_change_ends_the_search() {
    assert_eq!(parent_of(">   - ", &["- a"]), None);
    assert_eq!(parent_of(">   - ", &["", "> - a"]), None);
    assert_eq!(lead_of(">   - ", &["> - a"]).as_deref(), Some("> - "));
}

#[test]
fn top_level_item_has_no_parent() {
    assert_eq!(parent_of("- ", &["- a", "- b"]), None);
    assert_eq!(parent_of("- ", &[]), None);
}

#[test]
fn child_without_a_marker_has_no_parent() {
    assert_eq!(parent_of("text", &["- a"]), None);
}

#[test]
fn tab_indentation_compares_by_column() {
    assert_eq!(lead_of("\t- ", &["- a"]).as_deref(), Some("- "));
    assert_eq!(
        lead_of("\t\t- ", &["    - b", "- a"]).as_deref(),
        Some("    - ")
    );
}

#[test]
fn carriage_returns_are_tolerated() {
    assert_eq!(lead_of("  - ", &["- a\r"]).as_deref(), Some("- "));
}

#[test]
fn ordered_parent_is_found_with_its_number() {
    let parent = parent_of("   - ", &["3. a"]).unwrap();
    assert_eq!(parent.continuation().unwrap(), "4. ");
}

#[test]
fn parent_reports_how_many_lines_above_the_child_it_sits() {
    let child = parse_prefix("  - ");
    let above = ["  - b", "", "- a"];
    let parent = find_parent(&child, above.iter().copied()).unwrap();
    assert_eq!(parent.lines_above, 2);
}

#[test]
fn search_gives_up_after_the_scan_cap_even_through_skippable_lines() {
    let child = parse_prefix("  - ");
    let mut above = vec!["  more"; PARENT_SCAN_LINES];
    above.push("- a");
    assert_eq!(find_parent(&child, above.iter().copied()), None);

    above.remove(0);
    assert!(find_parent(&child, above.iter().copied()).is_some());
}
