#![allow(clippy::expect_used)]

use std::ops::Range;

use super::matcher::{MatchOptions, Matcher};

fn hits(query: &str, options: MatchOptions, hay: &str) -> Vec<Range<usize>> {
    Matcher::compile(query, options)
        .expect("the pattern compiles")
        .hits(hay)
}

const TEXT: MatchOptions = MatchOptions {
    case_sensitive: false,
    whole_word: false,
    regex: false,
};
const CASE: MatchOptions = MatchOptions {
    case_sensitive: true,
    ..TEXT
};
const WORD: MatchOptions = MatchOptions {
    whole_word: true,
    ..TEXT
};
const REGEX: MatchOptions = MatchOptions {
    regex: true,
    ..TEXT
};

#[test]
fn the_default_options_are_text_mode_case_insensitive_any_position() {
    assert_eq!(MatchOptions::default(), TEXT);
}

#[test]
fn a_text_query_with_regex_metacharacters_matches_literally() {
    assert_eq!(hits("a.b", TEXT, "a.b axb"), vec![0..3]);
    assert_eq!(hits("(x)", TEXT, "x (x)"), vec![2..5]);
}

#[test]
fn a_case_insensitive_query_finds_every_casing() {
    assert_eq!(hits("dog", TEXT, "Dog dog DOG"), vec![0..3, 4..7, 8..11]);
    assert_eq!(hits("café", TEXT, "CAFÉ menu"), vec![0..5]);
}

#[test]
fn a_case_sensitive_query_finds_only_its_own_casing() {
    assert_eq!(hits("dog", CASE, "Dog dog DOG"), vec![4..7]);
}

#[test]
fn whole_word_rejects_a_hit_inside_a_longer_word() {
    assert_eq!(hits("cat", WORD, "concatenate cat"), vec![12..15]);
}

#[test]
fn whole_word_and_regex_combine() {
    let options = MatchOptions {
        whole_word: true,
        ..REGEX
    };
    assert_eq!(hits("d.g", options, "dogs dog"), vec![5..8]);
}

#[test]
fn a_regex_query_matches_as_a_pattern() {
    assert_eq!(hits("d.g", REGEX, "dig dog"), vec![0..3, 4..7]);
}

#[test]
fn an_invalid_regex_reports_the_parse_error() {
    let error = Matcher::compile("(", REGEX).expect_err("an unclosed group is rejected");
    assert!(
        error.0.contains("regex parse error"),
        "the readout needs the parser's own words: {}",
        error.0
    );
}

#[test]
fn a_pattern_that_can_match_nothing_yields_no_zero_length_hits() {
    assert_eq!(hits("a*", REGEX, "baab"), vec![1..3]);
    assert!(hits("^", REGEX, "one\ntwo").is_empty());
}

#[test]
fn hits_are_non_overlapping_left_to_right() {
    assert_eq!(hits("aa", TEXT, "aaaa"), vec![0..2, 2..4]);
}

#[test]
fn an_empty_or_whitespace_only_query_compiles_and_matches_nothing() {
    assert!(hits("", TEXT, "hello world").is_empty());
    assert!(hits("   ", TEXT, "hello   world").is_empty());
    assert!(hits("   ", REGEX, "hello   world").is_empty());
}
