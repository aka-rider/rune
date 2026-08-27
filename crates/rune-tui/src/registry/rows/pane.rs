use crate::diff_view::keys::DiffCommand;
use crate::explorer_keys::ExplorerCommand;
use crate::explorer_search::ExplorerSearchCommand;
use crate::filesearch::keys::FileSearchCommand;
use crate::opentabs::TabsCommand;

use super::super::{ArgKind, CommandId, CommandSpec, always};

pub(crate) fn adapt_explorer(cmd: ExplorerCommand) -> CommandId {
    CommandId::Explorer(cmd)
}

pub(crate) fn adapt_explorer_search(cmd: ExplorerSearchCommand) -> CommandId {
    CommandId::ExplorerSearch(cmd)
}

pub(crate) fn adapt_tabs(cmd: TabsCommand) -> CommandId {
    CommandId::Tabs(cmd)
}

pub(crate) fn adapt_filesearch(cmd: FileSearchCommand) -> CommandId {
    CommandId::FileSearch(cmd)
}

pub(crate) fn adapt_diff(cmd: DiffCommand) -> CommandId {
    CommandId::Diff(cmd)
}

const fn explorer_row(cmd: ExplorerCommand, name: &'static str, help: &'static str) -> CommandSpec {
    CommandSpec {
        id: CommandId::Explorer(cmd),
        name,
        fuzzy_aliases: &[],
        help,
        arg: ArgKind::None,
        listed: false,
        availability: always,
    }
}

const fn explorer_search_row(
    cmd: ExplorerSearchCommand,
    name: &'static str,
    help: &'static str,
) -> CommandSpec {
    CommandSpec {
        id: CommandId::ExplorerSearch(cmd),
        name,
        fuzzy_aliases: &[],
        help,
        arg: ArgKind::None,
        listed: false,
        availability: always,
    }
}

const fn tabs_row(cmd: TabsCommand, name: &'static str, help: &'static str) -> CommandSpec {
    CommandSpec {
        id: CommandId::Tabs(cmd),
        name,
        fuzzy_aliases: &[],
        help,
        arg: ArgKind::None,
        listed: false,
        availability: always,
    }
}

const fn filesearch_row(
    cmd: FileSearchCommand,
    name: &'static str,
    help: &'static str,
) -> CommandSpec {
    CommandSpec {
        id: CommandId::FileSearch(cmd),
        name,
        fuzzy_aliases: &[],
        help,
        arg: ArgKind::None,
        listed: false,
        availability: always,
    }
}

const fn diff_row(cmd: DiffCommand, name: &'static str, help: &'static str) -> CommandSpec {
    CommandSpec {
        id: CommandId::Diff(cmd),
        name,
        fuzzy_aliases: &[],
        help,
        arg: ArgKind::None,
        listed: false,
        availability: always,
    }
}

pub(crate) static ROWS: &[CommandSpec] = &[
    explorer_row(ExplorerCommand::Up, "up", "up"),
    explorer_row(ExplorerCommand::Down, "down", "down"),
    explorer_row(ExplorerCommand::Top, "top", "top"),
    explorer_row(ExplorerCommand::Bottom, "bottom", "bottom"),
    explorer_row(ExplorerCommand::Open, "open", "open"),
    explorer_row(ExplorerCommand::ParentDir, "parent directory", "up dir"),
    explorer_row(ExplorerCommand::Leave, "leave explorer", "back to editor"),
    explorer_row(ExplorerCommand::Trash, "trash", "trash"),
    explorer_search_row(ExplorerSearchCommand::Type, "type", "search by name"),
    explorer_search_row(ExplorerSearchCommand::Erase, "erase", "erase search char"),
    explorer_search_row(
        ExplorerSearchCommand::Cancel,
        "cancel search",
        "cancel search",
    ),
    tabs_row(TabsCommand::Up, "up", "up"),
    tabs_row(TabsCommand::Down, "down", "down"),
    tabs_row(TabsCommand::Select, "select tab", "open"),
    tabs_row(TabsCommand::Leave, "leave tabs", "back to editor"),
    filesearch_row(FileSearchCommand::Type, "type", "type to filter"),
    filesearch_row(FileSearchCommand::Erase, "erase", "erase"),
    filesearch_row(FileSearchCommand::Up, "up", "up"),
    filesearch_row(FileSearchCommand::Down, "down", "down"),
    filesearch_row(FileSearchCommand::PageUp, "page up", "page up"),
    filesearch_row(FileSearchCommand::PageDown, "page down", "page down"),
    filesearch_row(FileSearchCommand::Top, "top", "top"),
    filesearch_row(FileSearchCommand::Bottom, "bottom", "bottom"),
    filesearch_row(FileSearchCommand::Enter, "open", "open"),
    filesearch_row(FileSearchCommand::Cancel, "cancel", "cancel"),
    diff_row(DiffCommand::NextHunk, "next hunk", "next hunk"),
    diff_row(DiffCommand::PrevHunk, "prev hunk", "prev hunk"),
    diff_row(DiffCommand::TakeTheirs, "take theirs", "take theirs"),
    diff_row(DiffCommand::TakeOurs, "take ours", "take ours"),
];
