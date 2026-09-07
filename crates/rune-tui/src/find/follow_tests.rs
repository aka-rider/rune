use std::ops::Range;

use rune_core::buffer::Buffer;
use rune_md::element::doc::DocMachine;
use rune_syntax::wrap::WrapSnapshot;

use super::follow::{at_or_after, concealed_ranges, is_concealed, step_index};

fn wrap_for(content: &str) -> WrapSnapshot {
    let buf = Buffer::new(content);
    let mut doc = DocMachine::new();
    doc.set_width(80);
    doc.sync_content(&buf);
    doc.snapshot(&buf).wrap.clone()
}

fn never(_: &Range<usize>) -> bool {
    false
}

#[test]
fn concealed_ranges_coalesces_adjacent_substituted_spans() {
    let wrap = wrap_for("| a | b |\n|---|---|\n| c | d |\n");
    let ranges = concealed_ranges(&wrap);
    assert!(
        !ranges.is_empty(),
        "a rendered table must substitute borders"
    );
    for pair in ranges.windows(2) {
        let (Some(a), Some(b)) = (pair.first(), pair.get(1)) else {
            continue;
        };
        assert!(
            a.end < b.start,
            "adjacent/overlapping ranges must have been coalesced: {ranges:?}"
        );
    }
}

#[test]
fn concealed_ranges_never_includes_identical_rendered_text() {
    let content = "**bold**\n";
    let wrap = wrap_for(content);
    let ranges = concealed_ranges(&wrap);
    let bold_at = content.find("bold").expect("fixture contains \"bold\"");
    let bold_range = bold_at..bold_at + "bold".len();
    assert!(
        !is_concealed(&ranges, &bold_range),
        "text rendered identically to its source must not be reported concealed: {ranges:?}"
    );
}

#[test]
fn a_match_straddling_a_concealed_table_border_edge_is_not_skipped() {
    let content = "| a | b |\n|---|---|\n| a | c |\n";
    let wrap = wrap_for(content);
    let concealed = concealed_ranges(&wrap);
    let range = concealed
        .first()
        .cloned()
        .expect("a rendered table must substitute at least one border range");
    assert!(
        range.end < content.len(),
        "fixture has room past the border"
    );

    let straddling = (range.end - 1)..(range.end + 1);
    assert!(
        !is_concealed(&concealed, &straddling),
        "a match that starts inside the concealed range but ends past it must stay navigable"
    );
}

#[test]
fn is_concealed_requires_full_containment_not_mere_overlap() {
    let ranges: Vec<Range<usize>> = Vec::from_iter(std::iter::once(10..20));
    assert!(is_concealed(&ranges, &(12..18)));
    assert!(is_concealed(&ranges, &(10..20)));
    assert!(!is_concealed(&ranges, &(5..15)), "straddles the left edge");
    assert!(
        !is_concealed(&ranges, &(15..25)),
        "straddles the right edge"
    );
    assert!(
        !is_concealed(&ranges, &(5..25)),
        "wholly overlaps, not contains"
    );
}

#[test]
fn stepping_forward_wraps_from_last_to_first() {
    let matches = vec![0..2, 10..12, 20..22];
    assert_eq!(step_index(&matches, 20, true, never), Some(0));
    assert_eq!(step_index(&matches, 0, true, never), Some(1));
    assert_eq!(step_index(&matches, 100, true, never), Some(0));
}

#[test]
fn stepping_backward_wraps_from_first_to_last() {
    let matches = vec![0..2, 10..12, 20..22];
    assert_eq!(step_index(&matches, 0, false, never), Some(2));
    assert_eq!(step_index(&matches, 20, false, never), Some(1));
}

#[test]
fn stepping_with_every_match_skipped_yields_none() {
    let matches = vec![0..2, 10..12];
    assert_eq!(step_index(&matches, 0, true, |_| true), None);
    assert_eq!(step_index(&matches, 0, false, |_| true), None);
}

#[test]
fn stepping_over_no_matches_yields_none() {
    let matches: Vec<Range<usize>> = Vec::new();
    assert_eq!(step_index(&matches, 0, true, never), None);
    assert_eq!(step_index(&matches, 0, false, never), None);
    assert_eq!(at_or_after(&matches, 0, never), None);
}

#[test]
fn stepping_forward_skips_concealed_matches() {
    let matches = vec![0..2, 10..12, 20..22];
    let skip = |r: &Range<usize>| *r == (10..12);
    assert_eq!(step_index(&matches, 0, true, skip), Some(2));
}

#[test]
fn at_or_after_prefers_the_match_under_the_origin_and_wraps_past_the_end() {
    let matches = vec![0..2, 10..12, 20..22];
    assert_eq!(at_or_after(&matches, 10, never), Some(1));
    assert_eq!(at_or_after(&matches, 11, never), Some(2));
    assert_eq!(at_or_after(&matches, 30, never), Some(0));
    assert_eq!(at_or_after(&matches, 0, |r| *r == (0..2)), Some(1));
}
