use std::path::PathBuf;

use rune_core::cursor::CursorSet;

use crate::app::App;
use crate::binding::{Binding, KeyPattern, resolve_in};
use crate::document::DocumentId;
use crate::keymap::{KeyCode, KeyInput, KeyOutcome, Mods};
use crate::pane::Pane;
use crate::queryline;
use crate::runtime::Effects;
use crate::viewport::ScrollMode;

use super::{cancel, close};

const SHIFT: Mods = Mods {
    shift: true,
    alt: false,
    ctrl: false,
    sup: false,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectSearchCommand {
    Type,
    Erase,
    Up,
    Down,
    PageUp,
    PageDown,
    Top,
    Bottom,
    Enter,
    Cancel,
}

pub const PROJECTSEARCH_BINDINGS: &[Binding<ProjectSearchCommand>] = &[
    Binding {
        key: KeyPattern::printable(Mods::NONE),
        cmd: ProjectSearchCommand::Type,
        help: "type to search",
        secondary: false,
    },
    Binding {
        key: KeyPattern::printable(SHIFT),
        cmd: ProjectSearchCommand::Type,
        help: "type to search",
        secondary: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Backspace, Mods::NONE),
        cmd: ProjectSearchCommand::Erase,
        help: "erase",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Up, Mods::NONE),
        cmd: ProjectSearchCommand::Up,
        help: "up",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Down, Mods::NONE),
        cmd: ProjectSearchCommand::Down,
        help: "down",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::PageUp, Mods::NONE),
        cmd: ProjectSearchCommand::PageUp,
        help: "page up",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::PageDown, Mods::NONE),
        cmd: ProjectSearchCommand::PageDown,
        help: "page down",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Home, Mods::NONE),
        cmd: ProjectSearchCommand::Top,
        help: "top",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::End, Mods::NONE),
        cmd: ProjectSearchCommand::Bottom,
        help: "bottom",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Enter, Mods::NONE),
        cmd: ProjectSearchCommand::Enter,
        help: "open",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Escape, Mods::NONE),
        cmd: ProjectSearchCommand::Cancel,
        help: "cancel",
        secondary: false,
    },
];

pub(crate) fn handle_key(app: &mut App, key: KeyInput, effects: &mut Effects) -> KeyOutcome {
    if let Some(cmd) = resolve_in(PROJECTSEARCH_BINDINGS, key) {
        apply(app, cmd, key, effects);
    }
    KeyOutcome::Consumed
}

fn apply(app: &mut App, cmd: ProjectSearchCommand, key: KeyInput, effects: &mut Effects) {
    match cmd {
        ProjectSearchCommand::Type => {
            if let (KeyCode::Char(c), Some(state)) = (key.code, app.projectsearch_mut()) {
                queryline::type_char(&mut state.query, c);
                super::restart_debounce(app);
            }
        }
        ProjectSearchCommand::Erase => {
            if let Some(state) = app.projectsearch_mut() {
                queryline::erase_grapheme(&mut state.query);
                super::restart_debounce(app);
            }
        }
        ProjectSearchCommand::Up => nav_move(app, -1, effects),
        ProjectSearchCommand::Down => nav_move(app, 1, effects),
        ProjectSearchCommand::PageUp => nav_move(app, -page_amount(app), effects),
        ProjectSearchCommand::PageDown => nav_move(app, page_amount(app), effects),
        ProjectSearchCommand::Top => nav_edge(app, true, effects),
        ProjectSearchCommand::Bottom => nav_edge(app, false, effects),
        ProjectSearchCommand::Enter => open_selected(app, effects),
        ProjectSearchCommand::Cancel => cancel(app, effects),
    }
}

pub(super) fn open_selected(app: &mut App, effects: &mut Effects) {
    let Some((path, first_match)) = selected_hit(app) else {
        crate::messages::info(app, "no file selected");
        return;
    };
    let departed = app.projectsearch().and_then(|state| state.return_to.raw());

    if let Some(id) = app.explorer.preview
        && app.doc(id).and_then(|d| d.file_path.as_deref()) == Some(path.as_path())
    {
        close(app);
        crate::explorer_preview::promote(app, id);
        app.set_focus_pane(Pane::Editor, effects);
        land_at(app, id, first_match);
        crate::navhistory::record_departure_if_moved(app, departed);
        return;
    }

    if let Some(id) = crate::workspace::open_path_checked(app, &path, effects) {
        close(app);
        app.set_focus_pane(Pane::Editor, effects);
        land_at(app, id, first_match);
        crate::navhistory::record_departure_if_moved(app, departed);
    }
}

fn selected_hit(app: &App) -> Option<(PathBuf, usize)> {
    let state = app.projectsearch()?;
    state
        .results
        .get(state.list.cursor)
        .map(|hit| (hit.path.clone(), hit.first_match))
}

fn land_at(app: &mut App, id: DocumentId, offset: usize) {
    let Some(doc) = app.doc_mut(id) else {
        return;
    };
    let clamped = offset.min(doc.buffer.content().len());
    doc.cursors = CursorSet::new(clamped);
    doc.viewport.mode = ScrollMode::EnsureVisible;
}

fn page_amount(app: &App) -> isize {
    crate::filesearch::keys::page_amount(app)
}

pub(super) fn nav_move(app: &mut App, delta: isize, effects: &mut Effects) {
    let height = page_amount(app).max(1) as usize;
    let Some(state) = app.projectsearch_mut() else {
        return;
    };
    let len = state.results.len();
    state.list.move_and_follow(delta, len, height);
    super::after_selection_change(app, effects);
}

fn nav_edge(app: &mut App, top: bool, effects: &mut Effects) {
    let Some(state) = app.projectsearch_mut() else {
        return;
    };
    let len = state.results.len();
    state.list.jump_to_edge(len, top);
    super::after_selection_change(app, effects);
}

pub(crate) fn paste(app: &mut App, text: &str) {
    let sanitized = queryline::sanitize_pasted_line(text);
    if sanitized.is_empty() {
        return;
    }
    if let Some(state) = app.projectsearch_mut() {
        state.query.push_str(&sanitized);
        super::restart_debounce(app);
    }
}
