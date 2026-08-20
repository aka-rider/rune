use std::ops::Range;

use rune_syntax::wrap::WrapSnapshot;

use crate::app::App;
use crate::document::DocumentId;
use crate::runtime::{CmdError, Effects};

pub(crate) mod keys;

pub(crate) struct SearchState {
    pub(crate) focused: bool,
    pub(crate) draft: String,
    pub(crate) matches: Vec<Range<usize>>,
    pub(crate) current: Option<usize>,
    pub(crate) doc: DocumentId,
    pub(crate) buffer_version: u64,
    pub(crate) history: Vec<String>,
    pub(crate) history_generation: crate::generation::SearchHistoryGen,
    pub(crate) history_pos: Option<usize>,
    pub(crate) history_draft: Option<String>,
}

pub(crate) fn open(app: &mut App, effects: &mut Effects) {
    if app.search().is_some() {
        return;
    }
    let Some(clearance) = app.clear_title_for_overlay(effects) else {
        return;
    };
    let history_generation = app.next_search_history_gen.mint();
    app.open_search(
        SearchState {
            focused: true,
            draft: String::new(),
            matches: Vec::new(),
            current: None,
            doc: app.active,
            buffer_version: app.active_doc().buffer.version(),
            history: Vec::new(),
            history_generation,
            history_pos: None,
            history_draft: None,
        },
        clearance,
    );
}

pub(crate) fn handle_history_loaded(
    app: &mut App,
    generation: crate::generation::SearchHistoryGen,
    result: Result<Vec<String>, CmdError>,
) {
    let current = app.search().map(|s| s.history_generation);
    if current != Some(generation) {
        return;
    }
    match result {
        Ok(entries) => {
            if let Some(state) = app.search_mut() {
                state.history = entries;
            }
        }
        Err(e) => {
            if let Some(state) = app.search_mut() {
                state.history = Vec::new();
            }
            crate::messages::error(app, format!("search history not loaded: {e}"));
        }
    }
}

pub(crate) fn close(app: &mut App) {
    let Some(state) = app.take_search() else {
        return;
    };
    if !state.draft.trim().is_empty() {
        app.last_search_query = Some(state.draft);
    }
}

pub(crate) fn recompute(app: &mut App) {
    if app.search().is_none() {
        return;
    }
    let draft = app.search().map(|s| s.draft.clone()).unwrap_or_default();
    let doc = app.active_doc();
    let matches = compute_matches(doc.buffer.content(), &draft);
    let version = doc.buffer.version();
    let doc_id = app.active;
    if let Some(state) = app.search_mut() {
        state.matches = matches;
        state.doc = doc_id;
        state.buffer_version = version;
        state.current = None;
    }
}

pub(crate) fn sync(app: &mut App) {
    let Some(state) = app.search() else {
        return;
    };
    let stale =
        state.doc != app.active || state.buffer_version != app.active_doc().buffer.version();
    if stale {
        recompute(app);
    }
}

fn fold_with_map(s: &str) -> (String, Vec<Range<usize>>) {
    let mut folded = String::new();
    let mut map = Vec::with_capacity(s.len());
    for (start, c) in s.char_indices() {
        let end = start + c.len_utf8();
        for lc in c.to_lowercase() {
            folded.push(lc);
            for _ in 0..lc.len_utf8() {
                map.push(start..end);
            }
        }
    }
    (folded, map)
}

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

pub(crate) fn concealed_ranges(wrap: &WrapSnapshot) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = wrap
        .segments()
        .iter()
        .flat_map(|seg| seg.spans.iter())
        .filter(|span| span.is_rendered())
        .map(rune_syntax::SyntaxSpan::range)
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

pub(crate) fn is_concealed(ranges: &[Range<usize>], m: &Range<usize>) -> bool {
    ranges.iter().any(|r| r.start <= m.start && m.end <= r.end)
}

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

fn is_subsequence(haystack: &str, needle: &[char]) -> bool {
    let mut chars = haystack.chars();
    needle.iter().all(|&nc| chars.any(|hc| hc == nc))
}

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
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
