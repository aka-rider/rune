use std::borrow::Cow;

use crate::app::App;
use crate::find::bindings::{FindCommand, label_for};
use crate::find::{Control, FilterKind, FindState};
use crate::footer_hints::HintEntry;
use crate::keymap::GlobalCommand;
use crate::registry::{self, CommandId};

pub(crate) fn entries(app: &App) -> Vec<HintEntry> {
    let Some(state) = app.find() else {
        return Vec::new();
    };
    let mut out = vec![row(FindCommand::Close)];
    let replace_expanded = state.replace.is_some();
    match (state.focus, state.filter_kind()) {
        (Control::Find, FilterKind::ThisFile) => {
            out.push(row(FindCommand::Commit));
            out.push(row(FindCommand::Alt));
            if replace_expanded {
                out.push(row_as(FindCommand::NextControl, "replace field"));
            } else {
                out.extend(global_row(GlobalCommand::ToggleReplace));
            }
            out.push(row(FindCommand::Up));
        }
        (Control::Find, FilterKind::Workspace) => {
            out.push(row_as(FindCommand::Commit, "open"));
            out.push(row_as(FindCommand::NextControl, next_control(state)));
            out.push(global_row_as(GlobalCommand::SearchNext, "next file"));
            out.push(row(FindCommand::Up));
            if !replace_expanded {
                out.extend(global_row(GlobalCommand::ToggleProjectReplace));
            }
        }
        (Control::Replace, _) => {
            out.push(row_as(FindCommand::Commit, "replace"));
            out.push(row_as(FindCommand::Alt, "replace all"));
            out.push(global_row_as(GlobalCommand::SearchNext, "skip"));
            out.push(row_as(FindCommand::NextControl, next_control(state)));
            out.push(row(FindCommand::Up));
        }
        (Control::Include | Control::Exclude, _) => {
            out.push(row_as(FindCommand::Commit, "open"));
            out.push(row_as(FindCommand::NextControl, next_control(state)));
        }
        (Control::Results, _) => {
            out.push(row_as(FindCommand::Commit, "open"));
            out.push(row_as(FindCommand::Down, "next"));
            out.push(row_as(FindCommand::Up, "previous"));
            out.push(row_as(FindCommand::PageDown, "page"));
            out.push(row_as(FindCommand::NextControl, next_control(state)));
        }
    }
    out
}

fn next_control(state: &FindState) -> &'static str {
    let ring = state.control_ring();
    let at = ring
        .iter()
        .position(|&control| control == state.focus)
        .unwrap_or(0);
    match ring.get((at + 1) % ring.len()) {
        Some(Control::Replace) => "replace field",
        Some(Control::Include) => "include",
        Some(Control::Exclude) => "exclude",
        Some(Control::Results) => "results",
        Some(Control::Find) | None => "find field",
    }
}

fn row(cmd: FindCommand) -> HintEntry {
    let help = registry::spec(CommandId::Find(cmd)).map_or("", |spec| spec.help);
    row_as(cmd, help)
}

fn row_as(cmd: FindCommand, help: &'static str) -> HintEntry {
    (label_for(cmd), Cow::Borrowed(help), true)
}

fn global_row(cmd: GlobalCommand) -> Option<HintEntry> {
    crate::global::hint_for(cmd).map(|(label, help)| (label, Cow::Borrowed(help), true))
}

fn global_row_as(cmd: GlobalCommand, help: &'static str) -> HintEntry {
    (crate::global::label_for(cmd), Cow::Borrowed(help), true)
}
