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
fn compute_matches_finds_hit_after_a_multibyte_char() {
    // 'é' folds to a single already-lowercase char but is 2 bytes in UTF-8;
    // the fold map must advance by that many bytes or every match after it
    // in the haystack indexes into the wrong place.
    let haystack = "café needle";
    assert_eq!(compute_matches(haystack, "needle"), vec![6..12]);
    assert_eq!(&haystack[6..12], "needle");
}

#[test]
fn compute_matches_finds_every_hit_after_a_multibyte_char() {
    let haystack = "notes — rune editor rune";
    let matches = compute_matches(haystack, "rune");
    assert_eq!(matches.len(), 2);
    for m in matches {
        assert_eq!(&haystack[m], "rune");
    }
}

#[test]
fn compute_matches_handles_non_ascii_query() {
    let haystack = "CAFÉ menu";
    assert_eq!(compute_matches(haystack, "café"), vec![0..5]);
    assert_eq!(&haystack[0..5], "CAF\u{c9}");
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
fn a_match_straddling_a_concealed_table_border_edge_is_not_skipped() {
    // Real markdown-derived concealed ranges (a rendered table border), not
    // hand-built internals — only the straddling MATCH range is synthetic.
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

/// Plan WP6.S2: a `Msg::SearchHistory` reply whose generation no longer
/// matches the CURRENTLY open bar's own `history_generation` — a
/// close-then-reopen since the load was issued — is dropped outright,
/// mirroring `explorer_dirload::handle_dir_loaded`'s own stale-generation
/// check.
#[test]
fn a_stale_generation_history_reply_is_discarded() {
    let mut app = app_with("hello");
    open(&mut app);
    let stale_generation = app.search.as_ref().unwrap().history_generation;
    close(&mut app);
    open(&mut app);
    let live_generation = app.search.as_ref().unwrap().history_generation;
    assert_ne!(
        stale_generation, live_generation,
        "each open mints a fresh generation"
    );

    handle_history_loaded(
        &mut app,
        stale_generation,
        Ok(vec!["should not land".into()]),
    );

    assert!(app.search.as_ref().unwrap().history.is_empty());
}

/// The matching positive case: a reply whose generation DOES match the
/// still-open bar adopts its entries.
#[test]
fn a_matching_generation_history_reply_populates_history() {
    let mut app = app_with("hello");
    open(&mut app);
    let generation = app.search.as_ref().unwrap().history_generation;

    handle_history_loaded(&mut app, generation, Ok(vec!["one".into(), "two".into()]));

    assert_eq!(
        app.search.as_ref().unwrap().history,
        vec!["one".to_string(), "two".to_string()]
    );
}

/// Plan WP6.S1: a reader failure degrades history to empty and reports
/// through the message log, but never closes the bar or otherwise disables
/// it — the search bar itself must keep working.
#[test]
fn a_reader_failure_degrades_history_to_empty_and_reports_a_message() {
    let mut app = app_with("hello");
    open(&mut app);
    let generation = app.search.as_ref().unwrap().history_generation;
    app.search.as_mut().unwrap().history = vec!["stale".to_string()];

    handle_history_loaded(&mut app, generation, Err("reader gone".to_string()));

    assert!(app.search.as_ref().unwrap().history.is_empty());
    assert!(app.search.is_some(), "the bar itself keeps working");
    assert!(crate::messages::newest_text(&app).is_some());
}
