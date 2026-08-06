//! The pure in-buffer match engine backing in-file search: case-insensitive
//! matching over the real buffer bytes, concealed-range collection so
//! navigation can skip a match hidden behind a substituted (decorated)
//! span, wraparound stepping through a match list, and a fuzzy filter for
//! browsing search history. Nothing here touches `App`, focus, layout, or
//! rendering — those land in a later work package.
//!
//! Every function below is exercised by this module's own tests but has no
//! production caller yet — the search bar that will call them lands in a
//! later work package. Scoped to non-test builds only: the test build
//! already proves each function reachable.
#![cfg_attr(not(test), allow(dead_code))]

use std::ops::Range;

use rune_syntax::wrap::WrapSnapshot;

/// Folds `s` char-by-char via `char::to_lowercase`, returning the folded
/// string together with a per-folded-BYTE map back to the original char's
/// `(start, end)` buffer byte range. A single original char can fold to
/// several chars (e.g. 'İ' U+0130 folds to "i\u{0307}"), so the map has one
/// entry per folded byte, all pointing at the same original range — that
/// lets a folded-string match spanning part of an expansion snap outward to
/// the whole original char it came from.
fn fold_with_map(s: &str) -> (String, Vec<Range<usize>>) {
    let mut folded = String::new();
    let mut map = Vec::with_capacity(s.len());
    for (start, c) in s.char_indices() {
        let end = start + c.len_utf8();
        for lc in c.to_lowercase() {
            folded.push(lc);
            map.push(start..end);
        }
    }
    (folded, map)
}

/// Every case-insensitive occurrence of `query` in `haystack`, as byte
/// ranges into `haystack` itself (never into the folded intermediate).
/// Matching folds both sides with `char::to_lowercase` and maps hits back
/// through [`fold_with_map`] — a hit ending mid-expansion snaps outward to
/// whole original chars, so every returned range sits on real char
/// boundaries. An empty or whitespace-only query yields no matches (a plain
/// `match_indices("")` would otherwise return a hit at every position).
///
/// This folds with `char::to_lowercase`, not full Unicode case-folding: two
/// strings that only case-fold equal by the fuller algorithm (e.g. "SS" and
/// "ß") will not match each other here.
pub(crate) fn compute_matches(haystack: &str, query: &str) -> Vec<Range<usize>> {
    if query.trim().is_empty() {
        return Vec::new();
    }
    let (folded_hay, map) = fold_with_map(haystack);
    let folded_query: String = query.chars().flat_map(char::to_lowercase).collect();
    if folded_query.is_empty() {
        return Vec::new();
    }
    folded_hay
        .match_indices(&folded_query)
        .filter_map(|(s, matched)| {
            let e = s + matched.len();
            let start = map.get(s)?.start;
            let end = map.get(e - 1)?.end;
            Some(start..end)
        })
        .collect()
}

/// The buffer byte ranges currently concealed behind a substituted
/// (decorated) span — table borders, list bullets, and the like — collected
/// from every wrap segment's spans, sorted, and coalesced so a logical span
/// sliced across several wrapped rows reads back as one range. A `Substituted`
/// span's visible text differs from what's at its buffer range, which is
/// exactly what makes landing a cursor there produce no visible match to
/// look at; an `Identical` span's visible text is a verbatim buffer slice,
/// so it never contributes a concealed range even when folded content (e.g.
/// the "bold" inside `**bold**`) sits next to spans that do.
pub(crate) fn concealed_ranges(wrap: &WrapSnapshot) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = wrap
        .segments()
        .iter()
        .flat_map(|seg| seg.spans.iter())
        .filter(|span| span.is_rendered())
        .map(|span| span.range())
        .collect();
    ranges.sort_by_key(|r| r.start);

    let mut coalesced: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for r in ranges {
        match coalesced.last_mut() {
            Some(last) if r.start <= last.end => {
                if r.end > last.end {
                    last.end = r.end;
                }
            }
            _ => coalesced.push(r),
        }
    }
    coalesced
}

/// True iff `m` lies fully inside one of `ranges` (containment, not mere
/// overlap) — a match straddling a concealed edge still has visible text to
/// land on, so it is navigable; only a match wholly swallowed by a
/// concealed range is skipped. `ranges` is assumed sorted and coalesced, as
/// [`concealed_ranges`] returns it.
pub(crate) fn is_concealed(ranges: &[Range<usize>], m: &Range<usize>) -> bool {
    ranges.iter().any(|r| r.start <= m.start && m.end <= r.end)
}

/// The index into `matches` (assumed sorted ascending by `start`, as
/// [`compute_matches`] returns it) of the first non-skipped match strictly
/// after `cursor_byte`, wrapping around to the front when the cursor is
/// past every match. `None` when `matches` is empty or every match is
/// skipped.
pub(crate) fn next_index(
    matches: &[Range<usize>],
    cursor_byte: usize,
    skip: impl Fn(&Range<usize>) -> bool,
) -> Option<usize> {
    let n = matches.len();
    if n == 0 {
        return None;
    }
    let start = matches
        .iter()
        .position(|m| m.start > cursor_byte)
        .unwrap_or(0);
    (0..n)
        .map(|offset| (start + offset) % n)
        .find(|&idx| matches.get(idx).is_some_and(|m| !skip(m)))
}

/// The wraparound mirror of [`next_index`]: the first non-skipped match
/// strictly before `cursor_byte`, walking backward and wrapping to the end.
pub(crate) fn prev_index(
    matches: &[Range<usize>],
    cursor_byte: usize,
    skip: impl Fn(&Range<usize>) -> bool,
) -> Option<usize> {
    let n = matches.len();
    if n == 0 {
        return None;
    }
    let start = matches
        .iter()
        .rposition(|m| m.start < cursor_byte)
        .unwrap_or(n - 1);
    (0..n)
        .map(|offset| (start + n - offset) % n)
        .find(|&idx| matches.get(idx).is_some_and(|m| !skip(m)))
}

/// Case-insensitive subsequence match: every char of `needle` (lowercased)
/// must appear in `haystack` (lowercased) in order, not necessarily
/// adjacent.
fn is_subsequence(haystack: &str, needle: &[char]) -> bool {
    let mut chars = haystack.chars();
    needle.iter().all(|&nc| chars.any(|hc| hc == nc))
}

/// History entries whose text contains `draft` as a case-insensitive
/// subsequence, preserving `history`'s own (MRU-first) order. An empty
/// draft returns every entry unfiltered.
pub(crate) fn fuzzy_filter<'a>(history: &'a [String], draft: &str) -> Vec<&'a String> {
    if draft.is_empty() {
        return history.iter().collect();
    }
    let needle: Vec<char> = draft.to_lowercase().chars().collect();
    history
        .iter()
        .filter(|entry| is_subsequence(&entry.to_lowercase(), &needle))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune_core::buffer::Buffer;
    use rune_md::element::doc::DocMachine;

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
}
