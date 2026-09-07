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
    let mut out: Vec<HintEntry> = Vec::new();
    if let Some((name, on)) = state.option(state.focus) {
        out.push((
            label_for(FindCommand::Activate),
            Cow::Owned(format!("{name}: {}", if on { "on" } else { "off" })),
            true,
        ));
    }
    out.push(row(FindCommand::Close));
    match (state.focus, state.scope()) {
        (Control::Find, Scope::File) => {
            out.push(row(FindCommand::Commit));
            out.push(row(FindCommand::Alt));
            out.push(row(FindCommand::Up));
            out.push(row_as(FindCommand::NextControl, "options"));
            if state.replace.is_none() {
                out.extend(global_row(GlobalCommand::ToggleReplace));
            }
        }
        (Control::Find, Scope::Project) => {
            out.push(row_as(FindCommand::Commit, "open"));
            out.push(row_as(FindCommand::PrevControl, "results"));
            out.push(row(FindCommand::Up));
            out.push(global_row_as(GlobalCommand::SearchNext, "next file"));
            if state.replace.is_none() {
                out.extend(global_row(GlobalCommand::ToggleProjectReplace));
            }
        }
        (Control::Results, _) => {
            out.push(row_as(FindCommand::Commit, "open"));
            out.push(row_as(FindCommand::Down, "next"));
            out.push(row_as(FindCommand::Up, "previous"));
            out.push(row_as(FindCommand::PageDown, "page"));
            out.push(row(FindCommand::NextControl));
        }
        (Control::Replace, _) => {
            out.push(row_as(FindCommand::Commit, "replace"));
            out.push(row_as(FindCommand::Alt, "replace all"));
            out.push(global_row_as(GlobalCommand::SearchNext, "skip"));
            out.push(row(FindCommand::NextControl));
        }
        (Control::ReplaceOne, _) => {
            out.push(row_as(FindCommand::Activate, "replace"));
            out.push(row_as(FindCommand::Alt, "replace all"));
            out.push(global_row_as(GlobalCommand::SearchNext, "skip"));
            out.push(row(FindCommand::NextControl));
        }
        (Control::ReplaceAll, _) => {
            out.push(row_as(FindCommand::Activate, "replace all"));
            out.push(global_row_as(GlobalCommand::SearchNext, "skip"));
            out.push(row(FindCommand::NextControl));
        }
        (Control::Scope, Scope::File) => {
            out.push(row_as(FindCommand::Activate, "search project"));
            out.push(row(FindCommand::NextControl));
        }
        (Control::Scope, Scope::Project) => {
            out.push(row_as(FindCommand::Activate, "search this file"));
            out.push(row(FindCommand::NextControl));
        }
        (Control::Case | Control::Word | Control::Regex, _) => {
            out.push(row(FindCommand::NextControl));
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
