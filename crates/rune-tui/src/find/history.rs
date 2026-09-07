use crate::app::App;
use crate::find::matcher::MatchOptions;
use crate::find::{Control, FieldState, follow};
use crate::messages;
use crate::runtime::CmdError;

pub(crate) fn handle_history_loaded(
    app: &mut App,
    generation: crate::generation::SearchHistoryGen,
    result: Result<Vec<String>, CmdError>,
) {
    let current = app.find().map(|s| s.history_generation);
    if current != Some(generation) {
        return;
    }
    match result {
        Ok(entries) => {
            if let Some(state) = app.find_mut() {
                state.find.history = entries;
            }
        }
        Err(e) => {
            if let Some(state) = app.find_mut() {
                state.find.history = Vec::new();
            }
            messages::error(app, format!("search history not loaded: {e}"));
        }
    }
}

pub(crate) fn persist_query(app: &mut App, query: &str, options: MatchOptions) {
    app.last_find = Some((query.to_string(), options));
    let result = app.search_history.touch(app.db.as_ref(), query, |db| {
        db.store.touch_search_query(query)
    });
    if let Some(Err(e)) = result {
        messages::error(app, format!("search history not saved: {e}"));
    }
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

#[derive(Clone, Copy)]
pub(crate) enum BrowseDir {
    Prev,
    Next,
}

fn browse_needle(field: &FieldState) -> String {
    field
        .history_draft
        .clone()
        .unwrap_or_else(|| field.draft.clone())
}

fn step_field(field: &mut FieldState, dir: BrowseDir) -> bool {
    if let BrowseDir::Next = dir {
        let Some(pos) = field.history_pos else {
            return false;
        };
        if pos == 0 {
            field.draft = field.history_draft.take().unwrap_or_default();
            field.history_pos = None;
            return true;
        }
    }

    let needle = browse_needle(field);
    let filtered: Vec<String> = fuzzy_filter(&field.history, &needle)
        .into_iter()
        .cloned()
        .collect();
    let next_pos = match dir {
        BrowseDir::Prev => field
            .history_pos
            .map_or(0, |pos| (pos + 1).min(filtered.len().saturating_sub(1))),
        BrowseDir::Next => field.history_pos.map_or(0, |pos| pos - 1),
    };
    let Some(entry) = filtered.get(next_pos).cloned() else {
        return false;
    };
    if let BrowseDir::Prev = dir {
        field.history_draft.get_or_insert(needle);
    }
    field.history_pos = Some(next_pos);
    field.draft = entry;
    true
}

pub(crate) fn step(app: &mut App, dir: BrowseDir) {
    let Some(state) = app.find_mut() else {
        return;
    };
    let focus = state.focus;
    let changed = state
        .focused_field_mut()
        .is_some_and(|field| step_field(field, dir));
    if changed && focus == Control::Find {
        follow::recompute(app);
        follow::follow(app);
    }
}
