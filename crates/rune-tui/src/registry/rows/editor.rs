use crate::app::App;
use crate::keymap::{Command, Extend, Motion};

use super::super::{ArgKind, Availability, CommandId, CommandSpec, always, read_only_edit, reload};

pub(crate) fn adapt(cmd: Command) -> CommandId {
    CommandId::Editor(cmd)
}

const fn row(cmd: Command, name: &'static str, help: &'static str, listed: bool) -> CommandSpec {
    availability_row(cmd, name, help, listed, always)
}

const fn edit_row(
    cmd: Command,
    name: &'static str,
    help: &'static str,
    listed: bool,
) -> CommandSpec {
    availability_row(cmd, name, help, listed, read_only_edit)
}

const fn availability_row(
    cmd: Command,
    name: &'static str,
    help: &'static str,
    listed: bool,
    availability: fn(&App) -> Availability,
) -> CommandSpec {
    CommandSpec {
        id: CommandId::Editor(cmd),
        name,
        fuzzy_aliases: &[],
        help,
        arg: ArgKind::None,
        listed,
        availability,
    }
}

pub(crate) static ROWS: &[CommandSpec] = &[
    row(
        Command::Motion(Motion::CharLeft, Extend::No),
        "move left",
        "move left",
        false,
    ),
    row(
        Command::Motion(Motion::CharRight, Extend::No),
        "move right",
        "move right",
        false,
    ),
    row(
        Command::Motion(Motion::CharLeft, Extend::Yes),
        "select char left",
        "select char left",
        false,
    ),
    row(
        Command::Motion(Motion::CharRight, Extend::Yes),
        "select char right",
        "select char right",
        false,
    ),
    row(
        Command::Motion(Motion::WordLeft, Extend::No),
        "word left",
        "word left",
        false,
    ),
    row(
        Command::Motion(Motion::WordRight, Extend::No),
        "word right",
        "word right",
        false,
    ),
    row(
        Command::Motion(Motion::WordLeft, Extend::Yes),
        "select word left",
        "select word left",
        false,
    ),
    row(
        Command::Motion(Motion::WordRight, Extend::Yes),
        "select word right",
        "select word right",
        false,
    ),
    row(
        Command::Motion(Motion::LineUp, Extend::No),
        "move up",
        "move up",
        false,
    ),
    row(
        Command::Motion(Motion::LineDown, Extend::No),
        "move down",
        "move down",
        false,
    ),
    row(
        Command::Motion(Motion::LineUp, Extend::Yes),
        "select line up",
        "select line up",
        false,
    ),
    row(
        Command::Motion(Motion::LineDown, Extend::Yes),
        "select line down",
        "select line down",
        false,
    ),
    edit_row(Command::MoveLineUp, "move line up", "move line up", true),
    edit_row(
        Command::MoveLineDown,
        "move line down",
        "move line down",
        true,
    ),
    edit_row(Command::CloneLineUp, "clone line up", "clone line up", true),
    edit_row(
        Command::CloneLineDown,
        "clone line down",
        "clone line down",
        true,
    ),
    edit_row(
        Command::AddCursorAbove,
        "cursor above",
        "cursor above",
        true,
    ),
    edit_row(
        Command::AddCursorBelow,
        "cursor below",
        "cursor below",
        true,
    ),
    row(
        Command::Motion(Motion::LineStart, Extend::No),
        "line start",
        "line start",
        false,
    ),
    row(
        Command::Motion(Motion::LineEnd, Extend::No),
        "line end",
        "line end",
        false,
    ),
    row(
        Command::Motion(Motion::LineStart, Extend::Yes),
        "select to line start",
        "select to line start",
        false,
    ),
    row(
        Command::Motion(Motion::LineEnd, Extend::Yes),
        "select to line end",
        "select to line end",
        false,
    ),
    row(
        Command::Motion(Motion::PageUp, Extend::No),
        "page up",
        "page up",
        false,
    ),
    row(
        Command::Motion(Motion::PageDown, Extend::No),
        "page down",
        "page down",
        false,
    ),
    row(
        Command::Motion(Motion::PageUp, Extend::Yes),
        "select page up",
        "select page up",
        false,
    ),
    row(
        Command::Motion(Motion::PageDown, Extend::Yes),
        "select page down",
        "select page down",
        false,
    ),
    edit_row(
        Command::Motion(Motion::MatchBracket, Extend::No),
        "jump to matching bracket",
        "jump to matching bracket",
        false,
    ),
    edit_row(
        Command::Motion(Motion::MatchBracket, Extend::Yes),
        "select to matching bracket",
        "select to matching bracket",
        false,
    ),
    edit_row(Command::DeleteLeft, "delete left", "delete left", false),
    edit_row(
        Command::DeleteWordLeft,
        "delete word left",
        "delete word left",
        false,
    ),
    edit_row(Command::DeleteRight, "delete right", "delete right", false),
    edit_row(
        Command::DeleteWordRight,
        "delete word right",
        "delete word right",
        false,
    ),
    edit_row(Command::Indent, "indent", "indent", true),
    edit_row(Command::Outdent, "outdent", "outdent", true),
    row(Command::SelectAll, "select all", "select all", true),
    row(Command::Copy, "copy", "copy", true),
    edit_row(Command::Cut, "cut", "cut", true),
    edit_row(Command::Paste, "paste", "paste", true),
    edit_row(Command::Undo, "undo", "undo", true),
    edit_row(Command::Redo, "redo", "redo", true),
    edit_row(Command::DeleteLine, "delete line", "delete line", true),
    row(Command::Save, "save", "save", false),
    row(
        Command::ScrollLineUp,
        "scroll line up",
        "scroll line up",
        false,
    ),
    row(
        Command::ScrollLineDown,
        "scroll line down",
        "scroll line down",
        false,
    ),
    row(
        Command::ScrollHalfPageUp,
        "scroll half page up",
        "scroll half page up",
        false,
    ),
    row(
        Command::ScrollHalfPageDown,
        "scroll half page down",
        "scroll half page down",
        false,
    ),
    row(
        Command::CentreCursor,
        "centre cursor",
        "centre cursor",
        true,
    ),
    row(
        Command::CursorToTop,
        "cursor to top of view",
        "cursor to top of view",
        true,
    ),
    row(
        Command::CursorToBottom,
        "cursor to bottom of view",
        "cursor to bottom of view",
        true,
    ),
    row(Command::FollowLink, "follow link", "follow link", true),
    availability_row(
        Command::Reload,
        "reload graphics",
        "reload graphics",
        true,
        reload,
    ),
    row(Command::QuitConfirm, "quit", "quit", false),
];
