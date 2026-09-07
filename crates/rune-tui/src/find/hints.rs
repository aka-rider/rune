use std::borrow::Cow;

use crate::app::App;
use crate::find::Control;
use crate::find::bindings::{FindCommand, label_for};
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
    match state.focus {
        Control::Find => {
            out.push(row(FindCommand::Commit));
            out.push(row(FindCommand::Alt));
            out.push(row(FindCommand::Up));
            out.push(row_as(FindCommand::NextControl, "options"));
            if state.replace.is_none()
                && let Some((label, help)) = crate::global::hint_for(GlobalCommand::ToggleReplace)
            {
                out.push((label, Cow::Borrowed(help), true));
            }
        }
        Control::Replace | Control::ReplaceOne | Control::ReplaceAll => {
            out.push(row(FindCommand::NextControl));
        }
        Control::Scope => {
            out.push(row_as(FindCommand::Activate, "project search"));
            out.push(row(FindCommand::NextControl));
        }
        Control::Case | Control::Word | Control::Regex => {
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
