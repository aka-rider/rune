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
mod replace_tests;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
pub(crate) mod test_support;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Control {
    Find,
    Scope,
    Case,
    Word,
    Regex,
    Replace,
    ReplaceOne,
    ReplaceAll,
}

impl Control {
    pub(crate) fn is_field(self) -> bool {
        matches!(self, Control::Find | Control::Replace)
    }
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
    pub history_generation: crate::generation::SearchHistoryGen,
    pub replace_history_generation: crate::generation::ReplaceHistoryGen,
}

impl FindState {
    pub(crate) fn focused_field_mut(&mut self) -> Option<&mut FieldState> {
        match self.focus {
            Control::Find => Some(&mut self.find),
            Control::Replace => self.replace.as_mut(),
            Control::Scope
            | Control::Case
            | Control::Word
            | Control::Regex
            | Control::ReplaceOne
            | Control::ReplaceAll => None,
        }
    }

    pub(crate) fn control_ring(&self) -> Vec<Control> {
        let mut ring = vec![
            Control::Find,
            Control::Scope,
            Control::Case,
            Control::Word,
            Control::Regex,
        ];
        if self.replace.is_some() {
            ring.extend([Control::Replace, Control::ReplaceOne, Control::ReplaceAll]);
        }
        ring
    }

    pub(crate) fn option(&self, control: Control) -> Option<(&'static str, bool)> {
        match control {
            Control::Case => Some(("Case", self.options.case_sensitive)),
            Control::Word => Some(("Word", self.options.whole_word)),
            Control::Regex => Some(("Regex", self.options.regex)),
            Control::Find
            | Control::Scope
            | Control::Replace
            | Control::ReplaceOne
            | Control::ReplaceAll => None,
        }
    }
}

pub(crate) fn open(app: &mut App, expand_replace: bool, effects: &mut Effects) {
    if app.find().is_some() {
        refocus(app, expand_replace, effects);
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
    if expand_replace {
        expand_replace_field(app, effects);
    }
    follow::recompute(app);
    follow::follow(app);
}

fn refocus(app: &mut App, expand_replace: bool, effects: &mut Effects) {
    if expand_replace {
        expand_replace_field(app, effects);
        return;
    }
    let Some(state) = app.find_mut() else {
        return;
    };
    if !state.focused || state.focus != Control::Find {
        state.focused = true;
        state.focus = Control::Find;
        return;
    }
    close(app, false);
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
    if restore {
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
