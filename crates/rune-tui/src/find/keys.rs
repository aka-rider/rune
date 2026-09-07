use ratatui::layout::Position;

use crate::app::App;
use crate::binding::resolve_in;
use crate::clipboard::pbpaste_cmd;
use crate::find::bindings::{FIND_BINDINGS, FindCommand, label_for};
use crate::find::history::{self, BrowseDir};
use crate::find::matcher::MatchOptions;
use crate::find::{Control, close, follow, replace};
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
    let Some(focus) = app.find().map(|state| state.focus) else {
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
        FindCommand::Commit => match focus {
            Control::Find => follow::advance(app, true),
            Control::Replace | Control::ReplaceOne => replace::replace_current(app),
            Control::ReplaceAll => replace::replace_all(app),
            Control::Scope | Control::Case | Control::Word | Control::Regex => {
                activate(app, focus, effects);
            }
        },
        FindCommand::Alt => match focus {
            Control::Find => follow::advance(app, false),
            Control::Replace | Control::ReplaceOne | Control::ReplaceAll => {
                replace::replace_all(app);
            }
            Control::Scope | Control::Case | Control::Word | Control::Regex => {
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
        FindCommand::Up => browse(app, focus, BrowseDir::Prev),
        FindCommand::Down => browse(app, focus, BrowseDir::Next),
    }
}

pub(crate) fn activate(app: &mut App, control: Control, effects: &mut Effects) {
    match control {
        Control::Find | Control::Replace => focus_control(app, control),
        Control::Scope => hand_off_to_project_search(app, effects),
        Control::Case => toggle_option(app, |o| &mut o.case_sensitive),
        Control::Word => toggle_option(app, |o| &mut o.whole_word),
        Control::Regex => toggle_option(app, |o| &mut o.regex),
        Control::ReplaceOne => replace::replace_current(app),
        Control::ReplaceAll => replace::replace_all(app),
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
        refollow(app);
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
    refollow(app);
}

fn refollow(app: &mut App) {
    follow::recompute(app);
    follow::follow(app);
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
                refollow(app);
            }
        }
        None => chip_hint(app),
    }
}

fn browse(app: &mut App, focus: Control, dir: BrowseDir) {
    if focus.is_field() {
        history::step(app, dir);
    } else {
        chip_hint(app);
    }
}

fn chip_hint(app: &mut App) {
    messages::info(
        app,
        format!("press {} to toggle", label_for(FindCommand::Activate)),
    );
}

fn hand_off_to_project_search(app: &mut App, effects: &mut Effects) {
    let draft = app
        .find()
        .map(|state| state.find.draft.clone())
        .unwrap_or_default();
    crate::projectsearch::open(app, effects);
    if let Some(state) = app.projectsearch_mut() {
        state.query = draft;
    }
    crate::projectsearch::restart_debounce(app);
}
