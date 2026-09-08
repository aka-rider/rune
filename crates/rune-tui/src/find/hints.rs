use std::borrow::Cow;

use crate::app::App;
use crate::find::bindings::{FindCommand, label_for};
use crate::find::{Control, Scope};
use crate::footer_hints::HintEntry;
use crate::keymap::GlobalCommand;
use crate::registry::{self, CommandId};

pub(crate) fn entries(app: &App) -> Vec<HintEntry> {
    let Some(state) = app.find() else {
        return Vec::new();
    };
    let mut out = vec![row(FindCommand::Close)];
    let replace_expanded = state.replace.is_some();
    match (state.focus, state.scope()) {
        (Control::Find, Scope::File) => {
            out.push(row(FindCommand::Commit));
            out.push(row(FindCommand::Alt));
            if replace_expanded {
                out.push(row_as(FindCommand::NextControl, "replace field"));
            } else {
                out.extend(global_row(GlobalCommand::ToggleReplace));
            }
            out.push(row(FindCommand::Up));
        }
        (Control::Find, Scope::Project) => {
            out.push(row_as(FindCommand::Commit, "open"));
            let next = if replace_expanded {
                "replace field"
            } else {
                "results"
            };
            out.push(row_as(FindCommand::NextControl, next));
            out.push(global_row_as(GlobalCommand::SearchNext, "next file"));
            out.push(row(FindCommand::Up));
            if !replace_expanded {
                out.extend(global_row(GlobalCommand::ToggleProjectReplace));
            }
        }
        (Control::Replace, scope) => {
            out.push(row_as(FindCommand::Commit, "replace"));
            out.push(row_as(FindCommand::Alt, "replace all"));
            out.push(global_row_as(GlobalCommand::SearchNext, "skip"));
            let next = if scope == Scope::Project {
                "results"
            } else {
                "find field"
            };
            out.push(row_as(FindCommand::NextControl, next));
            out.push(row(FindCommand::Up));
        }
        (Control::Results, _) => {
            out.push(row_as(FindCommand::Commit, "open"));
            out.push(row_as(FindCommand::Down, "next"));
            out.push(row_as(FindCommand::Up, "previous"));
            out.push(row_as(FindCommand::PageDown, "page"));
            out.push(row_as(FindCommand::NextControl, "find field"));
        }
    }
    out
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
