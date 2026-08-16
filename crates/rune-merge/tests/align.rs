#![allow(clippy::indexing_slicing, clippy::expect_used)]

use std::time::{Duration, Instant};

use rune_merge::{RegionKind, align, intraline};

#[test]
fn identical_inputs_are_one_same_region() {
    let text = "line1\nline2\nline3\n";
    let map = align(text, text);
    assert_eq!(map.regions.len(), 1);
    assert_eq!(map.regions[0].kind, RegionKind::Same);
    assert_eq!(map.regions[0].left_lines, 0..3);
    assert_eq!(map.regions[0].right_lines, 0..3);
}

#[test]
fn pure_insertion_is_right_only() {
    let left = "line1\nline3\n";
    let right = "line1\nline2\nline3\n";
    let map = align(left, right);
    let inserted = map
        .regions
        .iter()
        .find(|r| r.kind == RegionKind::RightOnly)
        .expect("expected a RightOnly region");
    assert_eq!(inserted.right_lines, 1..2);
    assert_eq!(inserted.left_lines, 1..1);
}

#[test]
fn regions_tile_both_inputs_with_no_gaps_or_overlaps() {
    let left = "a\nb\nc\nd\ne\n";
    let right = "a\nX\nc\nY\nZ\ne\n";
    let map = align(left, right);

    let mut left_cursor = 0usize;
    let mut right_cursor = 0usize;
    for region in &map.regions {
        assert_eq!(region.left_lines.start, left_cursor);
        assert_eq!(region.right_lines.start, right_cursor);
        left_cursor = region.left_lines.end;
        right_cursor = region.right_lines.end;
    }
    assert_eq!(left_cursor, 5);
    assert_eq!(right_cursor, 6);
}

#[test]
fn one_changed_word_is_emphasized_exactly_on_both_sides() {
    let left = "the quick fox\n";
    let right = "the slow fox\n";

    let map = align(left, right);
    assert_eq!(map.regions.len(), 1);
    assert_eq!(map.regions[0].kind, RegionKind::Changed);

    let spans = intraline(left, right, None);

    assert_eq!(spans.left.len(), 1);
    assert_eq!(spans.left[0].line, 0);
    assert_eq!(&left[spans.left[0].ranges[0].clone()], "quick");

    assert_eq!(spans.right.len(), 1);
    assert_eq!(spans.right[0].line, 0);
    assert_eq!(&right[spans.right[0].ranges[0].clone()], "slow");
}

fn heavily_reworded_line(prefix: &str) -> String {
    let mut line = String::new();
    for i in 0..5_000 {
        if i > 0 {
            line.push(' ');
        }
        if i % 3 == 0 {
            line.push_str(prefix);
            line.push_str(&i.to_string());
        } else {
            line.push_str("word");
            line.push_str(&i.to_string());
        }
    }
    line.push('\n');
    line
}

#[test]
fn elapsed_deadline_degrades_to_whole_line_emphasis() {
    let left = heavily_reworded_line("left");
    let right = heavily_reworded_line("right");
    let past = Instant::now().checked_sub(Duration::from_secs(3600));

    let spans = intraline(&left, &right, past);

    assert_eq!(spans.left.len(), 1);
    let left_covered: usize = spans.left[0].ranges.iter().map(|r| r.end - r.start).sum();
    assert_eq!(left_covered, left.len());

    assert_eq!(spans.right.len(), 1);
    let right_covered: usize = spans.right[0].ranges.iter().map(|r| r.end - r.start).sum();
    assert_eq!(right_covered, right.len());
}

#[test]
fn multibyte_content_produces_char_boundary_aligned_spans() {
    let left = "こんにちは 世界\n";
    let right = "こんにちは 🌍\n";

    let spans = intraline(left, right, None);

    assert_eq!(spans.left.len(), 1);
    for range in &spans.left[0].ranges {
        assert!(left.is_char_boundary(range.start));
        assert!(left.is_char_boundary(range.end));
    }
    assert_eq!(&left[spans.left[0].ranges[0].clone()], "世界");

    assert_eq!(spans.right.len(), 1);
    for range in &spans.right[0].ranges {
        assert!(right.is_char_boundary(range.start));
        assert!(right.is_char_boundary(range.end));
    }
    assert_eq!(&right[spans.right[0].ranges[0].clone()], "🌍");
}
