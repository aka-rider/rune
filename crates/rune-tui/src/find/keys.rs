use ratatui::layout::Position;

use crate::app::App;
use crate::binding::resolve_in;
use crate::clipboard::pbpaste_cmd;
use crate::find::bindings::{FIND_BINDINGS, FindCommand, label_for};
use crate::find::history::{self, BrowseDir};
use crate::find::matcher::MatchOptions;
use crate::find::{Control, Scope, close, follow, project, replace};
use crate::keymap::{self, Command, KeyCode, KeyInput, KeyOutcome};
use crate::layout_find::FindPanelGeometry;
use crate::messages;
use crate::queryline;
use crate::runtime::{Effects, PasteTarget};

pub(crate) fn handle_key(app: &mut App, key: KeyInput, effects: &mut Effects) -> KeyOutcome {
    if keymap::resolve(key) == Some(Command::Paste) {
        effects.cmds.push(pbpaste_cmd(PasteTarget::Find));
        return KeyOutcome::Consumed;
    }
    match resolve_in(FIND_BINDINGS, key) {
        Some(cmd) => apply(app, cmd, key, effects),
        None => messages::warn_if_new(
            app,
            format!(
                "key not bound in the find panel \u{2014} {} closes it",
                label_for(FindCommand::Close)
            ),
        ),
    }
    KeyOutcome::Consumed
}

fn apply(app: &mut App, cmd: FindCommand, key: KeyInput, effects: &mut Effects) {
    let Some((focus, scope)) = app.find().map(|state| (state.focus, state.scope())) else {
        return;
    };
    match cmd {
        FindCommand::Type => {
            if let KeyCode::Char(c) = key.code {
                type_char(app, c);
            }
        }
        FindCommand::Erase => erase(app),
        FindCommand::Close => close(app, true),
        FindCommand::Commit => match (focus, scope) {
            (Control::Find, Scope::File) => follow::advance(app, true),
            (Control::Find, Scope::Project) | (Control::Results, _) => {
                project::open_hit(app, effects);
            }
            (Control::Replace | Control::ReplaceOne, _) => replace::replace_current(app),
            (Control::ReplaceAll, _) => replace::replace_all(app),
            (Control::Scope | Control::Case | Control::Word | Control::Regex, _) => {
                activate(app, focus, effects);
            }
        },
        FindCommand::Alt => match (focus, scope) {
            (Control::Find, Scope::File) => follow::advance(app, false),
            (Control::Find, Scope::Project) => project::step_hit(app, false, effects),
            (Control::Results, _) => project::open_hit(app, effects),
            (Control::Replace | Control::ReplaceOne | Control::ReplaceAll, _) => {
                replace::replace_all(app);
            }
            (Control::Scope | Control::Case | Control::Word | Control::Regex, _) => {
                activate(app, focus, effects);
            }
        },
        FindCommand::NextControl => cycle(app, 1),
        FindCommand::PrevControl => cycle(app, -1),
        FindCommand::Activate => {
            if focus.is_field() {
                type_char(app, ' ');
            } else {
                activate(app, focus, effects);
            }
        }
        FindCommand::ToggleCase => toggle_option(app, |o| &mut o.case_sensitive),
        FindCommand::ToggleWord => toggle_option(app, |o| &mut o.whole_word),
        FindCommand::ToggleRegex => toggle_option(app, |o| &mut o.regex),
        FindCommand::Up => match focus {
            Control::Results => project::nav_move(app, -1, effects),
            _ => browse(app, focus, BrowseDir::Prev),
        },
        FindCommand::Down => match focus {
            Control::Results => project::nav_move(app, 1, effects),
            _ => browse(app, focus, BrowseDir::Next),
        },
        FindCommand::PageUp => list_move(app, focus, ListKey::PageUp, effects),
        FindCommand::PageDown => list_move(app, focus, ListKey::PageDown, effects),
        FindCommand::Home => list_move(app, focus, ListKey::Home, effects),
        FindCommand::End => list_move(app, focus, ListKey::End, effects),
    }
}

#[derive(Clone, Copy)]
enum ListKey {
    PageUp,
    PageDown,
    Home,
    End,
}

impl ListKey {
    fn command(self) -> FindCommand {
        match self {
            ListKey::PageUp => FindCommand::PageUp,
            ListKey::PageDown => FindCommand::PageDown,
            ListKey::Home => FindCommand::Home,
            ListKey::End => FindCommand::End,
        }
    }
}

pub(crate) fn activate(app: &mut App, control: Control, effects: &mut Effects) {
    match control {
        Control::Find | Control::Replace => focus_control(app, control),
        Control::Scope => project::toggle_scope(app, effects),
        Control::Case => toggle_option(app, |o| &mut o.case_sensitive),
        Control::Word => toggle_option(app, |o| &mut o.whole_word),
        Control::Regex => toggle_option(app, |o| &mut o.regex),
        Control::ReplaceOne => replace::replace_current(app),
        Control::ReplaceAll => replace::replace_all(app),
        Control::Results => project::open_hit(app, effects),
    }
}

pub(crate) fn click(
    app: &mut App,
    panel: &FindPanelGeometry,
    point: Position,
    effects: &mut Effects,
) {
    if let Some(state) = app.find_mut() {
        state.focused = true;
    }
    if panel.find_field.contains(point) {
        focus_control(app, Control::Find);
        return;
    }
    if panel.replace_field.is_some_and(|rect| rect.contains(point)) {
        focus_control(app, Control::Replace);
        return;
    }
    let hit = panel
        .chips
        .iter()
        .flatten()
        .find(|(_, rect)| rect.contains(point))
        .map(|(chip, _)| chip.control);
    if let Some(control) = hit {
        activate(app, control, effects);
    }
}

pub(crate) fn paste(app: &mut App, text: &str) {
    if app.find().is_none_or(|state| !state.focused) {
        return;
    }
    let sanitized = queryline::sanitize_pasted_line(text);
    if sanitized.is_empty() {
        return;
    }
    let Some(state) = app.find_mut() else {
        return;
    };
    let into_replace = state.focus == Control::Replace;
    let field = if into_replace {
        state.replace.as_mut()
    } else {
        Some(&mut state.find)
    };
    let Some(field) = field else {
        return;
    };
    field.draft.push_str(&sanitized);
    field.leave_history();
    if !into_replace {
        crate::find::requery(app);
    }
}

fn focus_control(app: &mut App, control: Control) {
    if let Some(state) = app.find_mut() {
        state.focused = true;
        state.focus = control;
    }
}

fn cycle(app: &mut App, delta: isize) {
    let Some(state) = app.find_mut() else {
        return;
    };
    let ring = state.control_ring();
    let len = ring.len() as isize;
    let pos = ring
        .iter()
        .position(|&control| control == state.focus)
        .unwrap_or(0) as isize;
    let next = (pos + delta).rem_euclid(len) as usize;
    if let Some(&control) = ring.get(next) {
        state.focus = control;
    }
}

fn toggle_option(app: &mut App, pick: fn(&mut MatchOptions) -> &mut bool) {
    let Some(state) = app.find_mut() else {
        return;
    };
    let flag = pick(&mut state.options);
    *flag = !*flag;
    crate::find::requery(app);
}

fn type_char(app: &mut App, c: char) {
    edit_focused_field(app, |draft| queryline::type_char(draft, c));
}

fn erase(app: &mut App) {
    edit_focused_field(app, queryline::erase_grapheme);
}

fn edit_focused_field(app: &mut App, edit: impl FnOnce(&mut String)) {
    let Some(state) = app.find_mut() else {
        return;
    };
    let focus = state.focus;
    match state.focused_field_mut() {
        Some(field) => {
            edit(&mut field.draft);
            field.leave_history();
            if focus == Control::Find {
                crate::find::requery(app);
            }
        }
        None => control_hint(app, focus),
    }
}

fn browse(app: &mut App, focus: Control, dir: BrowseDir) {
    if focus.is_field() {
        history::step(app, dir);
    } else {
        control_hint(app, focus);
    }
}

fn list_move(app: &mut App, focus: Control, key: ListKey, effects: &mut Effects) {
    if focus != Control::Results {
        list_key_hint(app, key.command());
        return;
    }
    let page = project::list_height(app) as isize;
    match key {
        ListKey::PageUp => project::nav_move(app, -page, effects),
        ListKey::PageDown => project::nav_move(app, page, effects),
        ListKey::Home => project::nav_edge(app, true, effects),
        ListKey::End => project::nav_edge(app, false, effects),
    }
}

fn control_hint(app: &mut App, focus: Control) {
    let text = if focus == Control::Results {
        format!(
            "press {} to open the result",
            label_for(FindCommand::Commit)
        )
    } else {
        format!("press {} to toggle", label_for(FindCommand::Activate))
    };
    messages::info(app, text);
}

fn list_key_hint(app: &mut App, cmd: FindCommand) {
    let text = if project::active(app) {
        format!(
            "{} moves the results \u{2014} {} reaches them",
            label_for(cmd),
            label_for(FindCommand::PrevControl)
        )
    } else {
        format!("{} moves the results in Project scope", label_for(cmd))
    };
    messages::info(app, text);
}
