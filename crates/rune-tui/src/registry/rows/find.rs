use crate::find::bindings::FindCommand;

use super::super::{ArgKind, CommandId, CommandSpec, always};

pub(crate) fn adapt(cmd: FindCommand) -> CommandId {
    CommandId::Find(cmd)
}

const fn row(cmd: FindCommand, name: &'static str, help: &'static str) -> CommandSpec {
    CommandSpec {
        id: CommandId::Find(cmd),
        name,
        fuzzy_aliases: &[],
        help,
        detail: "",
        arg: ArgKind::None,
        listed: false,
        availability: always,
    }
}

pub(crate) static ROWS: &[CommandSpec] = &[
    row(
        FindCommand::Type,
        "start typing to search",
        "type to search",
    ),
    row(FindCommand::Erase, "erase", "erase"),
    row(FindCommand::Close, "close the find panel", "close"),
    row(FindCommand::Commit, "go to next match", "next match"),
    row(FindCommand::Alt, "go to previous match", "previous match"),
    row(
        FindCommand::NextControl,
        "focus next control",
        "next control",
    ),
    row(
        FindCommand::PrevControl,
        "focus previous control",
        "previous control",
    ),
    row(
        FindCommand::ToggleScope,
        "toggle project scope",
        "toggle project scope",
    ),
    row(FindCommand::ToggleCase, "toggle match case", "match case"),
    row(FindCommand::ToggleWord, "toggle whole word", "whole word"),
    row(
        FindCommand::ToggleRegex,
        "toggle regular expression",
        "regex",
    ),
    row(FindCommand::Up, "recall older history", "history"),
    row(FindCommand::Down, "recall newer history", "newer"),
    row(
        FindCommand::PageUp,
        "go up a page of results",
        "results page up",
    ),
    row(
        FindCommand::PageDown,
        "go down a page of results",
        "results page down",
    ),
    row(FindCommand::Home, "go to first result", "first result"),
    row(FindCommand::End, "go to last result", "last result"),
];
