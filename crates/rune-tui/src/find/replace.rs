use rune_core::buffer::{AppliedEdit, Edit};
use rune_core::coords::{BufferOffset, VisualCol};
use rune_core::cursor::{Cursor, CursorId};
use rune_core::undo::EditKind;

use crate::app::App;
use crate::commands::edit_core::apply_edit_batch_with_cursors;
use crate::find::{Origin, follow, history};
use crate::messages;

pub(crate) fn replace_current(app: &mut App) {
    crate::find::sync(app);
    let Some(state) = app.find() else {
        return;
    };
    let Some(replacement) = state.replace.as_ref().map(|field| field.draft.clone()) else {
        return;
    };
    let Some(range) = state
        .current
        .and_then(|idx| state.matches.get(idx).cloned())
    else {
        select_next_or_report(app);
        return;
    };
    let Ok(matcher) = &state.pattern else {
        return;
    };
    let content = app.active_doc().buffer.content();
    let Some(insert) = matcher.replacement_at(content, &range, &replacement) else {
        return;
    };
    let query = state.find.draft.clone();
    let options = state.options;
    let edit = Edit {
        start: range.start,
        end: range.end,
        insert,
    };
    if apply(app, vec![edit]) == 0 {
        return;
    }
    history::persist_query(app, &query, options);
    history::persist_replacement(app, &replacement);
    follow::recompute(app);
    if app.find().is_some_and(|state| state.matches.is_empty()) {
        settle_origin(app);
        messages::info(app, "no more matches");
    } else {
        follow::advance(app, true);
    }
}

pub(crate) fn replace_all(app: &mut App) {
    crate::find::sync(app);
    let Some(state) = app.find() else {
        return;
    };
    let Some(replacement) = state.replace.as_ref().map(|field| field.draft.clone()) else {
        return;
    };
    let content = app.active_doc().buffer.content();
    let edits: Vec<Edit> = state
        .pattern
        .as_ref()
        .ok()
        .map(|matcher| matcher.replacements(content, &replacement))
        .unwrap_or_default()
        .into_iter()
        .map(|(range, insert)| Edit {
            start: range.start,
            end: range.end,
            insert,
        })
        .collect();
    let query = state.find.draft.clone();
    let options = state.options;
    if edits.is_empty() {
        messages::info(app, "no match to replace");
        return;
    }
    let count = apply(app, edits);
    if count == 0 {
        return;
    }
    let plural = if count == 1 { "" } else { "s" };
    messages::info(app, format!("replaced {count} occurrence{plural}"));
    history::persist_query(app, &query, options);
    history::persist_replacement(app, &replacement);
    follow::recompute(app);
    settle_origin(app);
}

fn select_next_or_report(app: &mut App) {
    if app.find().is_some_and(|state| state.matches.is_empty()) {
        messages::info(app, "no match to replace");
    } else {
        follow::advance(app, true);
    }
}

fn apply(app: &mut App, edits: Vec<Edit>) -> usize {
    let count = edits.len();
    let active = app.active;
    let cursors_before = app.active_doc().cursors.clone();
    let id = cursors_before.primary().id;
    let infos = edits.into_iter().map(|edit| (edit, id)).collect();
    let applied = apply_edit_batch_with_cursors(
        app,
        active,
        infos,
        &cursors_before,
        EditKind::Other,
        collapsed_after_the_last_edit,
    );
    if applied { count } else { 0 }
}

pub(crate) fn collapsed_after_the_last_edit(
    applied: &[AppliedEdit],
    ids: &[CursorId],
) -> Vec<Cursor> {
    let end = applied.first().map_or(0, |edit| edit.end);
    vec![Cursor {
        position: BufferOffset(end),
        anchor: BufferOffset(end),
        desired_col: VisualCol(0),
        id: ids.first().copied().unwrap_or(CursorId::FIRST),
    }]
}

fn settle_origin(app: &mut App) {
    let origin = Origin::capture(app);
    if let Some(state) = app.find_mut() {
        state.origin = origin;
    }
}
