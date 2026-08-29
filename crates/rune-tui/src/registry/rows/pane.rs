use crate::diff_view::keys::DiffCommand;
use crate::explorer_keys::ExplorerCommand;
use crate::explorer_search::ExplorerSearchCommand;
use crate::filesearch::keys::FileSearchCommand;
use crate::opentabs::TabsCommand;
use crate::palette::keys::PaletteKeyCommand;
use crate::projectsearch::keys::ProjectSearchCommand;

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

pub(crate) fn adapt_projectsearch(cmd: ProjectSearchCommand) -> CommandId {
    CommandId::ProjectSearch(cmd)
}

pub(crate) fn adapt_diff(cmd: DiffCommand) -> CommandId {
    CommandId::Diff(cmd)
}

pub(crate) fn adapt_palette_key(cmd: PaletteKeyCommand) -> CommandId {
    CommandId::PaletteKey(cmd)
}

const fn explorer_row(cmd: ExplorerCommand, name: &'static str, help: &'static str) -> CommandSpec {
    CommandSpec {
        id: CommandId::Explorer(cmd),
        name,
        fuzzy_aliases: &[],
        help,
        detail: "",
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
        detail: "",
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
        detail: "",
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
        detail: "",
        arg: ArgKind::None,
        listed: false,
        availability: always,
    }
}

const fn projectsearch_row(
    cmd: ProjectSearchCommand,
    name: &'static str,
    help: &'static str,
) -> CommandSpec {
    CommandSpec {
        id: CommandId::ProjectSearch(cmd),
        name,
        fuzzy_aliases: &[],
        help,
        detail: "",
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
        detail: "",
        arg: ArgKind::None,
        listed: false,
        availability: always,
    }
}

const fn palette_key_row(
    cmd: PaletteKeyCommand,
    name: &'static str,
    help: &'static str,
) -> CommandSpec {
    CommandSpec {
        id: CommandId::PaletteKey(cmd),
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
    explorer_row(ExplorerCommand::Up, "go to previous entry", "up"),
    explorer_row(ExplorerCommand::Down, "go to next entry", "down"),
    explorer_row(ExplorerCommand::Top, "go to first entry", "top"),
    explorer_row(ExplorerCommand::Bottom, "go to last entry", "bottom"),
    explorer_row(ExplorerCommand::Open, "open", "open"),
    explorer_row(
        ExplorerCommand::ParentDir,
        "go to parent directory",
        "up dir",
    ),
    explorer_row(ExplorerCommand::Leave, "leave explorer", "back to editor"),
    explorer_row(ExplorerCommand::Trash, "trash", "trash"),
    explorer_search_row(
        ExplorerSearchCommand::Type,
        "start typing to enter search",
        "search by name",
    ),
    explorer_search_row(ExplorerSearchCommand::Erase, "erase", "erase search char"),
    explorer_search_row(
        ExplorerSearchCommand::Cancel,
        "cancel search",
        "cancel search",
    ),
    tabs_row(TabsCommand::Up, "go to previous tab", "up"),
    tabs_row(TabsCommand::Down, "go to next tab", "down"),
    tabs_row(TabsCommand::Select, "select tab", "open"),
    tabs_row(TabsCommand::Leave, "leave tabs", "back to editor"),
    filesearch_row(
        FileSearchCommand::Type,
        "start typing to filter",
        "type to filter",
    ),
    filesearch_row(FileSearchCommand::Erase, "erase", "erase"),
    filesearch_row(FileSearchCommand::Up, "go to previous result", "up"),
    filesearch_row(FileSearchCommand::Down, "go to next result", "down"),
    filesearch_row(FileSearchCommand::PageUp, "go up a page", "page up"),
    filesearch_row(FileSearchCommand::PageDown, "go down a page", "page down"),
    filesearch_row(FileSearchCommand::Top, "go to first result", "top"),
    filesearch_row(FileSearchCommand::Bottom, "go to last result", "bottom"),
    filesearch_row(FileSearchCommand::Enter, "open the selected file", "open"),
    filesearch_row(FileSearchCommand::Cancel, "cancel", "cancel"),
    projectsearch_row(
        ProjectSearchCommand::Type,
        "start typing to search",
        "type to search",
    ),
    projectsearch_row(ProjectSearchCommand::Erase, "erase", "erase"),
    projectsearch_row(ProjectSearchCommand::Cancel, "cancel", "cancel"),
    diff_row(DiffCommand::NextHunk, "next hunk", "next hunk"),
    diff_row(DiffCommand::PrevHunk, "prev hunk", "prev hunk"),
    diff_row(DiffCommand::TakeTheirs, "take theirs", "take theirs"),
    diff_row(DiffCommand::TakeOurs, "take ours", "take ours"),
    palette_key_row(
        PaletteKeyCommand::Type,
        "start typing to filter",
        "type to filter",
    ),
    palette_key_row(PaletteKeyCommand::Erase, "erase", "erase"),
    palette_key_row(PaletteKeyCommand::Up, "go to previous entry", "up"),
    palette_key_row(PaletteKeyCommand::Down, "go to next entry", "down"),
    palette_key_row(PaletteKeyCommand::PageUp, "go up a page", "page up"),
    palette_key_row(PaletteKeyCommand::PageDown, "go down a page", "page down"),
    palette_key_row(PaletteKeyCommand::Top, "go to first entry", "top"),
    palette_key_row(PaletteKeyCommand::Bottom, "go to last entry", "bottom"),
    palette_key_row(PaletteKeyCommand::Enter, "run the command", "run"),
    palette_key_row(PaletteKeyCommand::Tab, "accept completion", "accept"),
    palette_key_row(PaletteKeyCommand::Cancel, "cancel", "cancel"),
];
