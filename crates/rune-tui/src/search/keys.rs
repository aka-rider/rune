use std::ops::Range;

use rune_core::cursor::CursorSet;

use crate::app::App;
use crate::clipboard::pbpaste_cmd;
use crate::keymap::{self, Command, KeyCode, KeyInput, KeyOutcome, Mods};
use crate::messages;
use crate::queryline;
use crate::runtime::{Effects, PasteTarget};
use crate::viewport::ScrollMode;

use super::{close, is_concealed, next_index, prev_index, recompute};

const SHIFT: Mods = Mods {
    shift: true,
    alt: false,
    ctrl: false,
    sup: false,
};

pub(crate) fn handle_key(app: &mut App, key: KeyInput, effects: &mut Effects) -> KeyOutcome {
    if keymap::resolve(key) == Some(Command::Paste) {
        effects.cmds.push(pbpaste_cmd(PasteTarget::Search));
        return KeyOutcome::Consumed;
    }
    match key.code {
        KeyCode::Escape => close(app),
        KeyCode::Backspace => erase(app),
        // Enter is a functional key, so SHIFT survives the kitty protocol,
        // unlike a shifted printable char.
        KeyCode::Enter if key.mods == Mods::NONE => advance(app, true),
        KeyCode::Enter if key.mods == SHIFT => advance(app, false),
        KeyCode::Up => history_prev(app),
        KeyCode::Down => history_next(app),
        KeyCode::Char(c) if !c.is_control() && (key.mods == Mods::NONE || key.mods == SHIFT) => {
            type_char(app, c);
        }
        _ => {}
    }
    KeyOutcome::Consumed
}

pub(crate) fn paste(app: &mut App, text: &str) {
    if app.search().is_none_or(|s| !s.focused) {
        return;
    }
    let sanitized = queryline::sanitize_pasted_line(text);
    if sanitized.is_empty() {
        return;
    }
    if let Some(state) = app.search_mut() {
        state.draft.push_str(&sanitized);
        state.history_pos = None;
        state.history_draft = None;
    }
    recompute(app);
}

pub(crate) fn advance(app: &mut App, forward: bool) {
    let Some(state) = app.search() else {
        return;
    };
    if state.doc != app.active || state.buffer_version != app.active_doc().buffer.version() {
        recompute(app);
    }
    let Some(state) = app.search() else {
        return;
    };
    let matches = state.matches.clone();
    let query = state.draft.clone();
    let concealed = current_concealed(app);
    match jump(app, &matches, &concealed, forward) {
        Some(idx) => {
            if let Some(s) = app.search_mut() {
                s.current = Some(idx);
            }
            persist_query(app, &query);
        }
        None => report_no_target(app, &query, &matches),
    }
}

pub(crate) fn advance_closed(app: &mut App, forward: bool) -> bool {
    let Some(query) = app.last_search_query.clone() else {
        return false;
    };
    let matches = super::compute_matches(app.active_doc().buffer.content(), &query);
    let concealed = current_concealed(app);
    match jump(app, &matches, &concealed, forward) {
        Some(_) => persist_query(app, &query),
        None => report_no_target(app, &query, &matches),
    }
    true
}

fn current_concealed(app: &App) -> Vec<Range<usize>> {
    app.active_doc()
        .view
        .as_ref()
        .map(|view| super::concealed_ranges(&view.wrap))
        .unwrap_or_default()
}

fn report_no_target(app: &mut App, query: &str, matches: &[Range<usize>]) {
    if matches.is_empty() {
        if !query.trim().is_empty() {
            messages::info(app, format!("no matches for \"{query}\""));
        }
    } else {
        messages::info(app, format!("all {} matches are concealed", matches.len()));
    }
}

fn jump(
    app: &mut App,
    matches: &[Range<usize>],
    concealed: &[Range<usize>],
    forward: bool,
) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    let cursor_byte = app.active_doc().cursors.primary().position.get();
    let idx = if forward {
        next_index(matches, cursor_byte, |r| is_concealed(concealed, r))
    } else {
        prev_index(matches, cursor_byte, |r| is_concealed(concealed, r))
    };
    let idx = idx?;
    let range = matches.get(idx)?.clone();
    let doc = app.active_doc_mut();
    doc.cursors = CursorSet::new(range.start);
    doc.viewport.mode = ScrollMode::EnsureVisible;
    Some(idx)
}

fn persist_query(app: &mut App, query: &str) {
    app.last_search_query = Some(query.to_string());
    let result = app.search_history.touch(app.db.as_ref(), query, |db| {
        db.store.touch_search_query(query)
    });
    if let Some(Err(e)) = result {
        messages::error(app, format!("search history not saved: {e}"));
    }
}

fn type_char(app: &mut App, c: char) {
    if let Some(state) = app.search_mut() {
        queryline::type_char(&mut state.draft, c);
        state.history_pos = None;
        state.history_draft = None;
    }
    recompute(app);
}

fn erase(app: &mut App) {
    if let Some(state) = app.search_mut() {
        state.history_pos = None;
        state.history_draft = None;
        queryline::erase_grapheme(&mut state.draft);
    }
    recompute(app);
}

fn browse_needle(state: &super::SearchState) -> String {
    state
        .history_draft
        .clone()
        .unwrap_or_else(|| state.draft.clone())
}

#[derive(Clone, Copy)]
enum BrowseDir {
    Prev,
    Next,
}

fn history_step(app: &mut App, dir: BrowseDir) {
    let Some(state) = app.search() else {
        return;
    };
    if let BrowseDir::Next = dir {
        let Some(pos) = state.history_pos else {
            return;
        };
        if pos == 0 {
            let restored = state.history_draft.clone().unwrap_or_default();
            let Some(state) = app.search_mut() else {
                return;
            };
            state.draft = restored;
            state.history_pos = None;
            state.history_draft = None;
            recompute(app);
            return;
        }
    }

    let needle = browse_needle(state);
    let filtered: Vec<String> = super::fuzzy_filter(&state.history, &needle)
        .into_iter()
        .cloned()
        .collect();
    let next_pos = match dir {
        BrowseDir::Prev => state
            .history_pos
            .map_or(0, |pos| (pos + 1).min(filtered.len().saturating_sub(1))),
        BrowseDir::Next => state.history_pos.map_or(0, |pos| pos - 1),
    };
    let Some(entry) = filtered.get(next_pos).cloned() else {
        return;
    };

    let Some(state) = app.search_mut() else {
        return;
    };
    if let BrowseDir::Prev = dir {
        state.history_draft.get_or_insert(needle);
    }
    state.history_pos = Some(next_pos);
    state.draft = entry;
    recompute(app);
}

fn history_prev(app: &mut App) {
    history_step(app, BrowseDir::Prev);
}

fn history_next(app: &mut App) {
    history_step(app, BrowseDir::Next);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod editing_tests;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod history_tests;
