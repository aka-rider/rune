//! Unit tests for the pure match engine and the state chokepoints in
//! `search/mod.rs` — split into its own file (500-line budget); a child
//! module of `search`, so every private item there stays reachable through
//! `use super::*;` exactly as if this were still inline.

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_md::element::doc::DocMachine;
use rune_vfs::Mem;

use super::*;

fn wrap_for(content: &str) -> WrapSnapshot {
    let buf = Buffer::new(content);
    let mut doc = DocMachine::new();
    doc.set_width(80);
    doc.sync_content(&buf);
    doc.snapshot(&buf).wrap
}

#[test]
fn compute_matches_finds_ascii_case_insensitive_hits() {
    let matches = compute_matches("Hello hello HELLO", "hello");
    assert_eq!(matches, vec![0..5, 6..11, 12..17]);
}

#[test]
fn compute_matches_snaps_expanding_fold_to_whole_original_char() {
    // 'İ' U+0130 folds to "i\u{0307}" (two chars) under `to_lowercase`;
    // matching just "i" must still return the whole original char's
    // byte range, not a byte offset that lands mid-expansion.
    let haystack = "\u{0130}stanbul";
    let matches = compute_matches(haystack, "i");
    assert_eq!(matches, vec![0.."\u{0130}".len()]);
}

#[test]
fn compute_matches_is_non_overlapping() {
    // `str::match_indices` semantics: overlapping occurrences are not
    // all reported, only the non-overlapping left-to-right ones.
    let matches = compute_matches("aaaa", "aa");
    assert_eq!(matches, vec![0..2, 2..4]);
}

#[test]
fn compute_matches_empty_or_whitespace_query_yields_nothing() {
    assert!(compute_matches("hello world", "").is_empty());
    assert!(compute_matches("hello world", "   ").is_empty());
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
            "adjacent/overlapping ranges must have been coalesced: {:?}",
            ranges
        );
    }
}

#[test]
fn concealed_ranges_never_includes_identical_folded_text() {
    // The delimiters around `**bold**` are substituted away, but "bold"
    // itself is an `Identical` span sliced verbatim from the buffer and
    // must stay navigable.
    let content = "**bold**\n";
    let wrap = wrap_for(content);
    let ranges = concealed_ranges(&wrap);
    let bold_at = content.find("bold");
    assert!(bold_at.is_some(), "fixture must contain \"bold\"");
    let bold_at = bold_at.unwrap_or(0);
    let bold_range = bold_at..bold_at + "bold".len();
    assert!(
        !is_concealed(&ranges, &bold_range),
        "folded Identical text must not be reported concealed: {:?}",
        ranges
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
fn next_index_wraps_from_last_to_first() {
    let matches = vec![0..2, 10..12, 20..22];
    let never_skip = |_: &Range<usize>| false;
    assert_eq!(next_index(&matches, 20, never_skip), Some(0));
    assert_eq!(next_index(&matches, 0, never_skip), Some(1));
    assert_eq!(next_index(&matches, 100, never_skip), Some(0));
}

#[test]
fn prev_index_wraps_from_first_to_last() {
    let matches = vec![0..2, 10..12, 20..22];
    let never_skip = |_: &Range<usize>| false;
    assert_eq!(prev_index(&matches, 0, never_skip), Some(2));
    assert_eq!(prev_index(&matches, 20, never_skip), Some(1));
}

#[test]
fn next_and_prev_index_skip_all_yields_none() {
    let matches = vec![0..2, 10..12];
    let skip_all = |_: &Range<usize>| true;
    assert_eq!(next_index(&matches, 0, skip_all), None);
    assert_eq!(prev_index(&matches, 0, skip_all), None);
}

#[test]
fn next_and_prev_index_on_empty_matches_is_none() {
    let matches: Vec<Range<usize>> = Vec::new();
    assert_eq!(next_index(&matches, 0, |_| false), None);
    assert_eq!(prev_index(&matches, 0, |_| false), None);
}

#[test]
fn next_index_skips_concealed_matches() {
    let matches = vec![0..2, 10..12, 20..22];
    let skip = |r: &Range<usize>| *r == (10..12);
    assert_eq!(next_index(&matches, 0, skip), Some(2));
}

#[test]
fn fuzzy_filter_is_case_insensitive_subsequence_preserving_mru_order() {
    let history = vec![
        "Readme Notes".to_string(),
        "todo list".to_string(),
        "REDO stack".to_string(),
    ];
    let hits: Vec<&str> = fuzzy_filter(&history, "rdo")
        .into_iter()
        .map(String::as_str)
        .collect();
    assert_eq!(hits, vec!["Readme Notes", "REDO stack"]);
}

#[test]
fn fuzzy_filter_empty_draft_returns_everything_unfiltered() {
    let history = vec!["a".to_string(), "b".to_string()];
    let hits: Vec<&str> = fuzzy_filter(&history, "")
        .into_iter()
        .map(String::as_str)
        .collect();
    assert_eq!(hits, vec!["a", "b"]);
}

fn app_with(content: &str) -> crate::app::App {
    crate::app::App::new(Buffer::new(content), None, Arc::new(Mem::new()), None)
}

#[test]
fn open_creates_a_focused_empty_draft_and_close_is_a_no_op_when_already_closed() {
    let mut app = app_with("hello");
    open(&mut app);
    let state = app.search.as_ref().expect("bar is open");
    assert!(state.focused);
    assert_eq!(state.draft, "");
    assert!(state.matches.is_empty());

    close(&mut app);
    assert!(app.search.is_none());
    // An empty query never overwrites `last_search_query` (there is
    // nothing to navigate back to).
    assert_eq!(app.last_search_query, None);

    // Closing an already-closed bar is a harmless no-op.
    close(&mut app);
    assert!(app.search.is_none());
}

#[test]
fn opening_twice_never_clobbers_an_in_progress_draft() {
    let mut app = app_with("hello");
    open(&mut app);
    app.search.as_mut().unwrap().draft.push('h');
    recompute(&mut app);

    open(&mut app);
    assert_eq!(app.search.as_ref().unwrap().draft, "h");
}

#[test]
fn switching_the_active_document_resets_the_match_set() {
    let mut app = app_with("hello hello");
    open(&mut app);
    app.search.as_mut().unwrap().draft = "hello".to_string();
    recompute(&mut app);
    assert_eq!(app.search.as_ref().unwrap().matches.len(), 2);

    let other = app.open_document(Buffer::new("no match in this one"));
    app.active = other;
    // `sync` (App::sync_view`'s own hook) is what notices the active
    // document changed underneath an already-open bar — nothing here
    // calls `recompute` directly, unlike a draft edit.
    sync(&mut app);

    assert!(app.search.as_ref().unwrap().matches.is_empty());
}
