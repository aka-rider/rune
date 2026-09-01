use crate::app::App;
use crate::global::GlobalCommand;
use crate::keymap::QuitKey;

use super::super::{
    ArgKind, Availability, CommandId, CommandSpec, PaletteCommand, always, merge, save,
    toggle_read_only,
};

pub(crate) fn adapt(cmd: GlobalCommand) -> CommandId {
    match cmd {
        GlobalCommand::TabSwitch(_) => CommandId::Palette(PaletteCommand::TabByName),
        GlobalCommand::QuitChord(_) => CommandId::Global(GlobalCommand::QuitChord(QuitKey::CtrlC)),
        other => CommandId::Global(other),
    }
}

const fn row(cmd: GlobalCommand, name: &'static str, help: &'static str) -> CommandSpec {
    full_row(cmd, name, help, &[], always)
}

const fn aliased_row(
    cmd: GlobalCommand,
    name: &'static str,
    help: &'static str,
    aliases: &'static [&'static str],
) -> CommandSpec {
    full_row(cmd, name, help, aliases, always)
}

const fn availability_row(
    cmd: GlobalCommand,
    name: &'static str,
    help: &'static str,
    availability: fn(&App) -> Availability,
) -> CommandSpec {
    full_row(cmd, name, help, &[], availability)
}

const fn full_row(
    cmd: GlobalCommand,
    name: &'static str,
    help: &'static str,
    fuzzy_aliases: &'static [&'static str],
    availability: fn(&App) -> Availability,
) -> CommandSpec {
    CommandSpec {
        id: CommandId::Global(cmd),
        name,
        fuzzy_aliases,
        help,
        detail: "",
        arg: ArgKind::None,
        listed: true,
        availability,
    }
}

pub(crate) static ROWS: &[CommandSpec] = &[
    aliased_row(
        GlobalCommand::ToggleLeft,
        "toggle explorer",
        "explorer",
        &["sidebar"],
    ),
    row(GlobalCommand::FocusTabs, "tabs", "tabs"),
    row(GlobalCommand::FocusTitle, "rename", "rename"),
    full_row(GlobalCommand::Save, "save", "save", &["write"], save),
    row(GlobalCommand::Help, "help", "help"),
    aliased_row(
        GlobalCommand::QuitChord(QuitKey::CtrlC),
        "quit",
        "quit",
        &["exit"],
    ),
    row(GlobalCommand::CloseFile, "close", "close"),
    row(GlobalCommand::NewDocument, "new", "new"),
    availability_row(
        GlobalCommand::ToggleReadOnly,
        "reading",
        "reading",
        toggle_read_only,
    ),
    availability_row(GlobalCommand::Merge, "merge", "merge", merge),
    row(GlobalCommand::ToggleMessages, "messages", "messages"),
    availability_row(
        GlobalCommand::Trash,
        "trash",
        "trash",
        crate::trash::availability,
    ),
    row(GlobalCommand::ToggleSearch, "search", "search"),
    row(GlobalCommand::SearchNext, "next match", "next match"),
    row(GlobalCommand::SearchPrev, "prev match", "prev match"),
    row(GlobalCommand::TogglePin, "pin", "pin"),
    row(GlobalCommand::ToggleFileSearch, "open file", "open file"),
    aliased_row(
        GlobalCommand::ToggleProjectSearch,
        "Search in Project",
        "search project",
        &["grep", "find in files"],
    ),
    aliased_row(
        GlobalCommand::TogglePalette,
        "command palette",
        "command palette",
        &["palette"],
    ),
    aliased_row(
        GlobalCommand::NavBack,
        "go back in history",
        "back",
        &["back"],
    ),
    aliased_row(
        GlobalCommand::NavForward,
        "go forward in history",
        "forward",
        &["forward"],
    ),
];
