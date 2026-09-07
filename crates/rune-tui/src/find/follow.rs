use std::ops::Range;

use rune_syntax::wrap::WrapSnapshot;

use crate::app::App;
use crate::commands::mouse::select_range;
use crate::find::matcher::Matcher;
use crate::find::{Origin, history};
use crate::messages;
use crate::viewport::ScrollMode;

pub(crate) fn recompute(app: &mut App) {
    let Some(state) = app.find() else {
        return;
    };
    let pattern = Matcher::compile(&state.find.draft, state.options);
    let doc = app.active_doc();
    let matches = pattern
        .as_ref()
        .map(|matcher| matcher.hits(doc.buffer.content()))
        .unwrap_or_default();
    let version = doc.buffer.version();
    let active = app.active;
    if let Some(state) = app.find_mut() {
        state.pattern = pattern;
        state.matches = matches;
        state.doc = active;
        state.buffer_version = version;
        state.current = None;
    }
}

pub(crate) fn follow(app: &mut App) {
    let Some(state) = app.find() else {
        return;
    };
    if state.project.is_some() {
        return;
    }
    let from = state.origin.cursors.primary().selection_start().get();
    let concealed = current_concealed(app);
    let found = at_or_after(&state.matches, from, |m| is_concealed(&concealed, m))
        .and_then(|idx| state.matches.get(idx).cloned().map(|m| (idx, m)));
    match found {
        Some((idx, range)) => {
            select_match(app, range);
            if let Some(state) = app.find_mut() {
                state.current = Some(idx);
            }
        }
        None => restore_to_origin(app),
    }
}

fn restore_to_origin(app: &mut App) {
    let Some(state) = app.find() else {
        return;
    };
    let origin = Origin {
        doc: state.origin.doc,
        cursors: state.origin.cursors.clone(),
        scroll_row: state.origin.scroll_row,
    };
    crate::find::restore_origin(app, &origin);
}

pub(crate) fn advance(app: &mut App, forward: bool) {
    crate::find::sync(app);
    let Some(state) = app.find() else {
        return;
    };
    let query = state.find.draft.clone();
    let options = state.options;
    let match_count = state.matches.len();
    let concealed = current_concealed(app);
    let cursor = app.active_doc().cursors.primary().selection_start().get();
    let found = step_index(&state.matches, cursor, forward, |m| {
        is_concealed(&concealed, m)
    })
    .and_then(|idx| state.matches.get(idx).cloned().map(|m| (idx, m)));
    match found {
        Some((idx, range)) => {
            select_match(app, range);
            commit_origin(app, idx);
            history::persist_query(app, &query, options);
        }
        None => report_no_target(app, &query, match_count),
    }
}

pub(crate) fn advance_closed(app: &mut App, forward: bool) -> bool {
    let Some((query, options)) = app.last_find.clone() else {
        return false;
    };
    let matches = Matcher::compile(&query, options)
        .map(|matcher| matcher.hits(app.active_doc().buffer.content()))
        .unwrap_or_default();
    let concealed = current_concealed(app);
    let cursor = app.active_doc().cursors.primary().selection_start().get();
    let found = step_index(&matches, cursor, forward, |m| is_concealed(&concealed, m))
        .and_then(|idx| matches.get(idx).cloned());
    match found {
        Some(range) => {
            select_match(app, range);
            history::persist_query(app, &query, options);
        }
        None => report_no_target(app, &query, matches.len()),
    }
    true
}

fn select_match(app: &mut App, range: Range<usize>) {
    let doc = app.active_doc_mut();
    select_range(doc, range.start, range.end);
    doc.viewport.mode = ScrollMode::EnsureVisible;
}

fn commit_origin(app: &mut App, idx: usize) {
    let doc = app.active_doc();
    let origin = Origin {
        doc: app.active,
        cursors: doc.cursors.clone(),
        scroll_row: doc.viewport.scroll_row,
    };
    if let Some(state) = app.find_mut() {
        state.origin = origin;
        state.current = Some(idx);
    }
}

fn report_no_target(app: &mut App, query: &str, match_count: usize) {
    if match_count == 0 {
        if !query.trim().is_empty() {
            messages::info(app, format!("no matches for \"{query}\""));
        }
    } else {
        messages::info(app, format!("all {match_count} matches are concealed"));
    }
}

pub(crate) fn current_concealed(app: &App) -> Vec<Range<usize>> {
    app.active_doc()
        .view
        .as_ref()
        .map(|view| concealed_ranges(&view.wrap))
        .unwrap_or_default()
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

fn wrapping_scan(
    matches: &[Range<usize>],
    start: usize,
    forward: bool,
    skip: impl Fn(&Range<usize>) -> bool,
) -> Option<usize> {
    let n = matches.len();
    (0..n)
        .map(|offset| {
            if forward {
                (start + offset) % n
            } else {
                (start + n - offset) % n
            }
        })
        .find(|&idx| matches.get(idx).is_some_and(|m| !skip(m)))
}

pub(crate) fn at_or_after(
    matches: &[Range<usize>],
    from: usize,
    skip: impl Fn(&Range<usize>) -> bool,
) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    let start = matches.iter().position(|m| m.start >= from).unwrap_or(0);
    wrapping_scan(matches, start, true, skip)
}

pub(crate) fn step_index(
    matches: &[Range<usize>],
    cursor_byte: usize,
    forward: bool,
    skip: impl Fn(&Range<usize>) -> bool,
) -> Option<usize> {
    let n = matches.len();
    if n == 0 {
        return None;
    }
    let start = if forward {
        matches
            .iter()
            .position(|m| m.start > cursor_byte)
            .unwrap_or(0)
    } else {
        matches
            .iter()
            .rposition(|m| m.start < cursor_byte)
            .unwrap_or(n - 1)
    };
    wrapping_scan(matches, start, forward, skip)
}
