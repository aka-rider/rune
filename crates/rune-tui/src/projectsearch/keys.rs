use crate::app::App;
use crate::binding::{Binding, KeyPattern, resolve_in};
use crate::keymap::{KeyCode, KeyInput, KeyOutcome, Mods};
use crate::queryline;
use crate::runtime::Effects;

use super::cancel;

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
            }
        }
        ProjectSearchCommand::Erase => {
            if let Some(state) = app.projectsearch_mut() {
                queryline::erase_grapheme(&mut state.query);
            }
        }
        ProjectSearchCommand::Cancel => cancel(app, effects),
    }
}

pub(crate) fn paste(app: &mut App, text: &str) {
    let sanitized = queryline::sanitize_pasted_line(text);
    if sanitized.is_empty() {
        return;
    }
    if let Some(state) = app.projectsearch_mut() {
        state.query.push_str(&sanitized);
    }
}
