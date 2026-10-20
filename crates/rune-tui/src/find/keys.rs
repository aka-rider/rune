use ratatui::layout::Position;

use crate::app::App;
use crate::binding::resolve_in;
use crate::clipboard::pbpaste_cmd;
use crate::find::bindings::{FIND_BINDINGS, FindCommand, label_for};
use crate::find::history::{self, BrowseDir};
use crate::find::matcher::MatchOptions;
use crate::find::{
    ChipKind, Control, FieldState, FilterKind, FindState, escape, follow, project, project_replace,
    replace, scope,
};
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
    let panel_cmd = resolve_in(FIND_BINDINGS, key);
    if field_focused(app) && defers_to_editor(panel_cmd) && try_field_key(app, key) {
        return KeyOutcome::Consumed;
    }
    match panel_cmd {
        Some(cmd) => apply(app, cmd, effects),
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

// Home/End/Erase/Type all have a panel-command reading (page the results,
// erase, type-to-search) that only applies once a field can no longer
// claim the key for itself — a field takes first refusal on exactly these
// four, and on anything the table has no row for at all.
fn defers_to_editor(cmd: Option<FindCommand>) -> bool {
    matches!(
        cmd,
        None | Some(FindCommand::Home | FindCommand::End | FindCommand::Erase | FindCommand::Type)
    )
}

fn field_focused(app: &App) -> bool {
    app.find().is_some_and(|state| state.focus.is_field())
}

// Mirrors `title::keys::handle_key`'s own two-step resolve: a motion,
// selection, delete, or undo command goes to `TextField::apply`; anything
// EDITOR_BINDINGS has no row for, that is still a plain typed character,
// goes to `TextField::insert`. Copy/Cut are rejected outright — unlike the
// title, a find field has no clipboard path of its own (paste already
// arrives through `paste` below) — so those two fall through unhandled
// and land on the same "key not bound" hint any other unbound chord gets.
fn try_field_key(app: &mut App, key: KeyInput) -> bool {
    let Some(focus) = app.find().map(|state| state.focus) else {
        return false;
    };
    if let Some(cmd) = keymap::resolve_in(keymap::editor_bindings::EDITOR_BINDINGS, key)
        && !matches!(cmd, Command::Copy | Command::Cut)
    {
        commit_field_edit(app, focus, |field| {
            let window = 0..field.editor.len();
            let _ = field.editor.apply(cmd, window);
        });
        true
    } else if let KeyCode::Char(ch) = key.code
        && !key.mods.ctrl
        && !key.mods.alt
        && !key.mods.sup
    {
        commit_field_edit(app, focus, |field| {
            let window = 0..field.editor.len();
            let _ = field.editor.insert(&ch.to_string(), window);
        });
        true
    } else {
        false
    }
}

// The one chokepoint every field mutation funnels through: it leaves
// whatever history browse was in progress, then — for the Find field —
// reruns the same recompute/follow/debounce-restart path
// `crate::find::requery` always ran after a keystroke, so matches never
// drift from what the field displays; an Include or Exclude edit only
// changes which files are queried, so it restarts the debounce alone.
fn commit_field_edit(app: &mut App, target: Control, edit: impl FnOnce(&mut FieldState)) {
    let Some(state) = app.find_mut() else {
        return;
    };
    let Some(field) = state.field_mut(target) else {
        return;
    };
    edit(field);
    field.leave_history();
    match target {
        Control::Find => crate::find::requery(app),
        Control::Include | Control::Exclude => {
            if let Some(state) = app.find_mut() {
                state.glob_fields.refresh();
            }
            project::restart_debounce(app);
        }
        Control::Replace | Control::Results => {}
    }
}

fn apply(app: &mut App, cmd: FindCommand, effects: &mut Effects) {
    let Some((focus, filter)) = app.find().map(|state| (state.focus, state.filter_kind())) else {
        return;
    };
    match cmd {
        FindCommand::Type | FindCommand::Erase => control_hint(app),
        FindCommand::Close => {
            escape(app);
        }
        FindCommand::Commit => match (focus, filter) {
            (Control::Include | Control::Exclude, _) => project::open_hit(app, effects),
            (Control::Find, FilterKind::ThisFile) => follow::advance(app, true),
            (Control::Find, FilterKind::Workspace) | (Control::Results, _) => {
                project::open_hit(app, effects);
            }
            (Control::Replace, _) => replace_one(app, filter, effects),
        },
        FindCommand::Alt => match (focus, filter) {
            (Control::Include | Control::Exclude, _) => project::step_hit(app, false, effects),
            (Control::Find, FilterKind::ThisFile) => follow::advance(app, false),
            (Control::Find, FilterKind::Workspace) => project::step_hit(app, false, effects),
            (Control::Results, _) => project::open_hit(app, effects),
            (Control::Replace, _) => replace_every(app, filter, effects),
        },
        FindCommand::NextControl => cycle(app, 1),
        FindCommand::PrevControl => cycle(app, -1),
        FindCommand::ToggleThisFile => project::toggle_this_file(app, effects),
        FindCommand::ToggleCase => toggle_option(app, |o| &mut o.case_sensitive),
        FindCommand::ToggleWord => toggle_option(app, |o| &mut o.whole_word),
        FindCommand::ToggleRegex => toggle_option(app, |o| &mut o.regex),
        FindCommand::ToggleHistoryScope => scope::toggle(app, effects),
        FindCommand::Up => match focus {
            Control::Results => project::nav_move(app, -1, effects),
            Control::Find | Control::Replace => history::step(app, BrowseDir::Prev),
            Control::Include | Control::Exclude => glob_history_hint(app),
        },
        FindCommand::Down => match focus {
            Control::Results => project::nav_move(app, 1, effects),
            Control::Find | Control::Replace => history::step(app, BrowseDir::Next),
            Control::Include | Control::Exclude => glob_history_hint(app),
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

pub(crate) fn press_chip(app: &mut App, kind: ChipKind, effects: &mut Effects) {
    let Some(filter) = app.find().map(FindState::filter_kind) else {
        return;
    };
    match kind {
        ChipKind::ThisFile => project::toggle_this_file(app, effects),
        ChipKind::Case => toggle_option(app, |o| &mut o.case_sensitive),
        ChipKind::Word => toggle_option(app, |o| &mut o.whole_word),
        ChipKind::Regex => toggle_option(app, |o| &mut o.regex),
        ChipKind::HistoryScope => scope::toggle(app, effects),
        ChipKind::ReplaceOne => replace_one(app, filter, effects),
        ChipKind::ReplaceAll => replace_every(app, filter, effects),
    }
}

fn replace_one(app: &mut App, filter: FilterKind, effects: &mut Effects) {
    match filter {
        FilterKind::ThisFile => replace::replace_current(app),
        FilterKind::Workspace => project_replace::replace_selected(app, effects),
    }
}

fn replace_every(app: &mut App, filter: FilterKind, effects: &mut Effects) {
    match filter {
        FilterKind::ThisFile => replace::replace_all(app),
        FilterKind::Workspace => project_replace::replace_all_project(app, effects),
    }
}

pub(crate) fn click(
    app: &mut App,
    panel: &FindPanelGeometry,
    point: Position,
    effects: &mut Effects,
) {
    if !crate::find::take_focus(app, effects) {
        return;
    }
    if panel.find_field.contains(point) {
        focus_control(app, Control::Find);
        return;
    }
    if panel.replace_field.is_some_and(|rect| rect.contains(point)) {
        focus_control(app, Control::Replace);
        return;
    }
    if panel.include_field.is_some_and(|rect| rect.contains(point)) {
        focus_control(app, Control::Include);
        return;
    }
    if panel.exclude_field.is_some_and(|rect| rect.contains(point)) {
        focus_control(app, Control::Exclude);
        return;
    }
    let hit = panel
        .chips
        .iter()
        .flatten()
        .find(|(_, rect)| rect.contains(point))
        .map(|(chip, _)| chip.kind);
    if let Some(kind) = hit {
        press_chip(app, kind, effects);
    }
}

// Targets the focused text field; Results has no text field of its own to
// receive a paste, so it lands in Find.
pub(crate) fn paste(app: &mut App, text: &str) {
    if app.find().is_none_or(|state| !state.focused) {
        return;
    }
    let sanitized = queryline::sanitize_pasted_line(text);
    if sanitized.is_empty() {
        return;
    }
    let target = match app.find().map(|state| state.focus) {
        Some(focus) if focus.is_field() => focus,
        Some(_) | None => Control::Find,
    };
    commit_field_edit(app, target, |field| {
        let window = 0..field.editor.len();
        let _ = field.editor.insert(&sanitized, window);
    });
}

fn focus_control(app: &mut App, control: Control) {
    if let Some(state) = app.find_mut() {
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

fn control_hint(app: &mut App) {
    let text = format!(
        "press {} to open the result",
        label_for(FindCommand::Commit)
    );
    messages::info(app, text);
}

fn glob_history_hint(app: &mut App) {
    messages::info(
        app,
        "path globs keep no history \u{2014} separate several patterns with commas",
    );
}

fn list_key_hint(app: &mut App, cmd: FindCommand) {
    let text = if project::listed(app) {
        format!(
            "{} moves the results \u{2014} {} reaches them",
            label_for(cmd),
            label_for(FindCommand::PrevControl)
        )
    } else {
        format!(
            "{} moves the results list, shown when 2+ files match the workspace search ({} toggles This file)",
            label_for(cmd),
            label_for(FindCommand::ToggleThisFile)
        )
    };
    messages::info(app, text);
}
