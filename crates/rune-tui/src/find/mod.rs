use crate::app::App;
use crate::document::Document;
use crate::find::matcher::{MatchOptions, Matcher};
use crate::keymap::GlobalCommand;
use crate::messages;
use crate::runtime::Effects;
use crate::viewport::ScrollMode;

pub(crate) mod bindings;
pub(crate) mod follow;
pub(crate) mod globs;
pub(crate) mod hints;
pub(crate) mod history;
pub(crate) mod keys;
pub(crate) mod land;
pub(crate) mod matcher;
pub(crate) mod project;
pub(crate) mod project_replace;
pub(crate) mod replace;
pub(crate) mod scope;
pub(crate) mod state;

pub(crate) use state::{ChipKind, Control, FieldState, Filter, FilterKind, FindState, Origin};

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod controls_tests;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod esc_tests;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod field_edit_tests;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod filter_tests;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod follow_tests;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod glob_tests;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod history_tests;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod keys_tests;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod land_tests;
#[cfg(test)]
mod matcher_tests;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod project_index_tests;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod project_preview_tests;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod project_query_tests;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
pub(crate) mod project_replace_fixture;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod project_replace_limit_tests;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod project_replace_tests;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod project_tests;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod replace_tests;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod scope_tests;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
pub(crate) mod test_support;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests;

pub(crate) fn open(app: &mut App, kind: FilterKind, expand_replace: bool, effects: &mut Effects) {
    if app.find().is_some() {
        refocus(app, kind, expand_replace, effects);
        return;
    }
    if matches!(app.merge, crate::merge::MergeState::Active { doc, .. } if doc == app.active) {
        let merge_key = crate::global::label_for(GlobalCommand::Merge);
        messages::info(app, format!("finish the merge first ({merge_key})"));
        return;
    }
    let Some(clearance) = app.clear_title_for_overlay(effects) else {
        return;
    };
    app.close_all_overlays(effects);
    let origin = Origin::capture(app);
    let seed = selection_seed(app.active_doc()).unwrap_or_default();
    let history_generation = app.next_search_history_gen.mint();
    let replace_history_generation = app.next_replace_history_gen.mint();
    app.open_find(
        FindState {
            focused: true,
            options: MatchOptions::default(),
            focus: Control::Find,
            find: FieldState::seeded(seed),
            replace: None,
            pattern: Matcher::compile("", MatchOptions::default()),
            origin,
            doc: app.active,
            buffer_version: app.active_doc().buffer.version(),
            matches: Vec::new(),
            current: None,
            filter: Filter::ThisFile,
            glob_fields: globs::GlobFields::default(),
            history_generation,
            replace_history_generation,
        },
        clearance,
    );
    scope::request_history(app, history_generation, effects);
    project::apply_filter(app, kind, effects);
    if expand_replace {
        expand_replace_field(app, effects);
    }
    requery(app);
}

fn refocus(app: &mut App, kind: FilterKind, expand_replace: bool, effects: &mut Effects) {
    let Some(current) = app.find().map(FindState::filter_kind) else {
        return;
    };
    let switched = current != kind;
    if switched {
        project::set_filter(app, kind, effects);
    }
    if expand_replace {
        expand_replace_field(app, effects);
        return;
    }
    let Some(state) = app.find() else {
        return;
    };
    if switched || !state.focused || state.focus != Control::Find {
        take_focus(app, effects);
        if let Some(state) = app.find_mut() {
            state.focus = Control::Find;
        }
        return;
    }
    close(app, false);
}

pub(crate) fn requery(app: &mut App) {
    follow::recompute(app);
    follow::follow(app);
    project::restart_debounce(app);
}

fn expand_replace_field(app: &mut App, effects: &mut Effects) {
    take_focus(app, effects);
    let Some(state) = app.find_mut() else {
        return;
    };
    state.focus = Control::Replace;
    if state.replace.is_some() {
        return;
    }
    state.replace = Some(FieldState::default());
    let generation = state.replace_history_generation;
    if let Some(db) = app.db.as_ref() {
        effects.cmds.push(crate::runtime::load_replace_history_cmd(
            db.store.reader_query(),
            generation,
        ));
    }
}

fn selection_seed(doc: &Document) -> Option<String> {
    let (start, end) = doc.cursors.primary().selection_range();
    let text = doc.buffer.slice(start.get(), end.get())?;
    (!text.is_empty() && !text.contains('\n')).then(|| text.to_string())
}

pub(crate) fn close(app: &mut App, restore: bool) {
    let Some(state) = app.take_find() else {
        return;
    };
    let find_text = state.find.editor.text().to_string();
    if !find_text.trim().is_empty() {
        app.last_find = Some((find_text, state.options));
    }
    if let Filter::Workspace(results) = state.filter {
        crate::explorer_preview::discard(app);
        project_replace::abandon(app, results.walk);
    }
    if restore {
        if app.active != state.origin.doc {
            crate::workspace::switch_to(app, state.origin.doc);
        }
        restore_origin(app, &state.origin);
    }
}

pub(crate) fn escape(app: &mut App) -> bool {
    let Some(focused) = app.find().map(|state| state.focused) else {
        return false;
    };
    close(app, focused);
    true
}

pub(crate) fn take_focus(app: &mut App, effects: &mut Effects) {
    if app.find().is_none() {
        return;
    }
    app.blur_title(effects);
    if let Some(state) = app.find_mut() {
        state.focused = true;
    }
}

pub(crate) fn accept_location(app: &mut App) {
    let origin = Origin::capture(app);
    if let Some(state) = app.find_mut() {
        state.focused = false;
        state.origin = origin;
    }
}

pub(crate) fn navigating_as_panel<R>(app: &mut App, navigate: impl FnOnce(&mut App) -> R) -> R {
    let kept = app
        .find()
        .map(|state| (state.focused, state.origin.clone()));
    let result = navigate(app);
    if let (Some((focused, origin)), Some(state)) = (kept, app.find_mut()) {
        state.focused = focused;
        state.origin = origin;
    }
    result
}

pub(crate) fn restore_origin(app: &mut App, origin: &Origin) {
    let Some(doc) = app.doc_mut(origin.doc) else {
        return;
    };
    if doc.buffer.version() != origin.buffer_version {
        return;
    }
    doc.cursors = origin.cursors.clone();
    doc.viewport.scroll_row = origin.scroll_row;
    doc.viewport.mode = ScrollMode::EnsureVisible;
}

pub(crate) fn sync(app: &mut App) {
    let Some(state) = app.find() else {
        return;
    };
    let stale =
        state.doc != app.active || state.buffer_version != app.active_doc().buffer.version();
    if stale {
        follow::recompute(app);
    }
}

pub(crate) fn reveal_offsets(app: &App) -> Vec<usize> {
    app.find()
        .filter(|state| state.doc == app.active)
        .map(|state| {
            state
                .matches
                .iter()
                .flat_map(|m| [m.start, m.end.saturating_sub(1).max(m.start)])
                .collect()
        })
        .unwrap_or_default()
}
