use std::ops::Range;

use rune_core::coords::DisplayRow;
use rune_core::cursor::CursorSet;

use crate::app::App;
use crate::document::{Document, DocumentId};
use crate::find::matcher::{MatchOptions, Matcher, PatternError};
use crate::keymap::GlobalCommand;
use crate::messages;
use crate::runtime::Effects;
use crate::viewport::ScrollMode;

pub(crate) mod bindings;
pub(crate) mod follow;
pub(crate) mod hints;
pub(crate) mod history;
pub(crate) mod keys;
pub(crate) mod matcher;
pub(crate) mod project;
pub(crate) mod project_replace;
pub(crate) mod replace;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod controls_tests;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod follow_tests;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod history_tests;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod keys_tests;
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
pub(crate) mod test_support;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scope {
    File,
    Project,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Control {
    Find,
    Replace,
    Results,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChipKind {
    Scope,
    Case,
    Word,
    Regex,
    ReplaceOne,
    ReplaceAll,
}

#[derive(Debug, Default)]
pub(crate) struct FieldState {
    pub draft: String,
    pub history: Vec<String>,
    pub history_pos: Option<usize>,
    pub history_draft: Option<String>,
}

impl FieldState {
    fn seeded(draft: String) -> FieldState {
        FieldState {
            draft,
            ..FieldState::default()
        }
    }

    pub(crate) fn leave_history(&mut self) {
        self.history_pos = None;
        self.history_draft = None;
    }
}

pub(crate) struct Origin {
    pub doc: DocumentId,
    pub cursors: CursorSet,
    pub scroll_row: DisplayRow,
}

impl Origin {
    pub(crate) fn capture(app: &App) -> Origin {
        let doc = app.active_doc();
        Origin {
            doc: app.active,
            cursors: doc.cursors.clone(),
            scroll_row: doc.viewport.scroll_row,
        }
    }
}

pub(crate) struct FindState {
    pub focused: bool,
    pub options: MatchOptions,
    pub focus: Control,
    pub find: FieldState,
    pub replace: Option<FieldState>,
    pub pattern: Result<Matcher, PatternError>,
    pub origin: Origin,
    pub doc: DocumentId,
    pub buffer_version: u64,
    pub matches: Vec<Range<usize>>,
    pub current: Option<usize>,
    pub project: Option<project::ProjectResults>,
    pub history_generation: crate::generation::SearchHistoryGen,
    pub replace_history_generation: crate::generation::ReplaceHistoryGen,
}

impl FindState {
    pub(crate) fn scope(&self) -> Scope {
        if self.project.is_some() {
            Scope::Project
        } else {
            Scope::File
        }
    }

    pub(crate) fn focused_field_mut(&mut self) -> Option<&mut FieldState> {
        match self.focus {
            Control::Find => Some(&mut self.find),
            Control::Replace => self.replace.as_mut(),
            Control::Results => None,
        }
    }

    pub(crate) fn control_ring(&self) -> Vec<Control> {
        let mut ring = vec![Control::Find];
        if self.replace.is_some() {
            ring.push(Control::Replace);
        }
        if self.project.is_some() {
            ring.push(Control::Results);
        }
        ring
    }

    pub(crate) fn chip_on(&self, kind: ChipKind) -> Option<bool> {
        match kind {
            ChipKind::Case => Some(self.options.case_sensitive),
            ChipKind::Word => Some(self.options.whole_word),
            ChipKind::Regex => Some(self.options.regex),
            ChipKind::Scope | ChipKind::ReplaceOne | ChipKind::ReplaceAll => None,
        }
    }
}

pub(crate) fn open(app: &mut App, scope: Scope, expand_replace: bool, effects: &mut Effects) {
    if app.find().is_some() {
        refocus(app, scope, expand_replace, effects);
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
            project: None,
            history_generation,
            replace_history_generation,
        },
        clearance,
    );
    if let Some(db) = app.db.as_ref() {
        effects.cmds.push(crate::runtime::load_search_history_cmd(
            db.store.reader_query(),
            history_generation,
        ));
    }
    if scope == Scope::Project {
        project::attach(app, effects);
    }
    if expand_replace {
        expand_replace_field(app, effects);
    }
    requery(app);
}

fn refocus(app: &mut App, scope: Scope, expand_replace: bool, effects: &mut Effects) {
    let Some(current) = app.find().map(FindState::scope) else {
        return;
    };
    let switched = current != scope;
    if switched {
        project::set_scope(app, scope, effects);
    }
    if expand_replace {
        expand_replace_field(app, effects);
        return;
    }
    let Some(state) = app.find_mut() else {
        return;
    };
    if switched || !state.focused || state.focus != Control::Find {
        state.focused = true;
        state.focus = Control::Find;
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
    let Some(state) = app.find_mut() else {
        return;
    };
    state.focused = true;
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
    if !state.find.draft.trim().is_empty() {
        app.last_find = Some((state.find.draft, state.options));
    }
    if let Some(project) = state.project {
        crate::explorer_preview::discard(app);
        project_replace::abandon(app, project.walk);
    }
    if restore {
        if app.active != state.origin.doc {
            crate::workspace::switch_to(app, state.origin.doc);
        }
        restore_origin(app, &state.origin);
    }
}

pub(crate) fn unfocus(app: &mut App) {
    if let Some(state) = app.find_mut() {
        state.focused = false;
    }
}

pub(crate) fn restore_origin(app: &mut App, origin: &Origin) {
    let Some(doc) = app.doc_mut(origin.doc) else {
        return;
    };
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
