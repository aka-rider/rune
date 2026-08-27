use crate::binding::{Binding, KeyPattern};
use crate::keymap::{Command, KeyCode};

use super::{ALT, ALT_SUP, CTRL, NONE, SHIFT, SHIFT_ALT, SUP, SUP_SHIFT};

pub(crate) const MOVE_LINE_UP: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Up, ALT),
    cmd: Command::MoveLineUp,
    help: "move line up",
    secondary: false,
};

pub(crate) const MOVE_LINE_DOWN: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Down, ALT),
    cmd: Command::MoveLineDown,
    help: "move line down",
    secondary: false,
};

pub(crate) const CLONE_LINE_UP: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Up, SHIFT_ALT),
    cmd: Command::CloneLineUp,
    help: "clone line up",
    secondary: false,
};

pub(crate) const CLONE_LINE_DOWN: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Down, SHIFT_ALT),
    cmd: Command::CloneLineDown,
    help: "clone line down",
    secondary: false,
};

pub(crate) const ADD_CURSOR_ABOVE: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Up, ALT_SUP),
    cmd: Command::AddCursorAbove,
    help: "cursor above",
    secondary: false,
};

pub(crate) const ADD_CURSOR_BELOW: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Down, ALT_SUP),
    cmd: Command::AddCursorBelow,
    help: "cursor below",
    secondary: false,
};

pub(crate) const DELETE_LEFT: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Backspace, NONE),
    cmd: Command::DeleteLeft,
    help: "delete left",
    secondary: false,
};

pub(crate) const DELETE_WORD_LEFT: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Backspace, ALT),
    cmd: Command::DeleteWordLeft,
    help: "delete word left",
    secondary: false,
};

pub(crate) const DELETE_RIGHT: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Delete, NONE),
    cmd: Command::DeleteRight,
    help: "delete right",
    secondary: false,
};

pub(crate) const DELETE_WORD_RIGHT: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Delete, ALT),
    cmd: Command::DeleteWordRight,
    help: "delete word right",
    secondary: false,
};

pub(crate) const INDENT: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Tab, NONE),
    cmd: Command::Indent,
    help: "indent",
    secondary: false,
};

pub(crate) const OUTDENT_SHIFT_TAB: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Tab, SHIFT),
    cmd: Command::Outdent,
    help: "outdent",
    secondary: false,
};

pub(crate) const UNDO_SUP: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Char('z'), SUP),
    cmd: Command::Undo,
    help: "undo",
    secondary: false,
};

pub(crate) const UNDO_CTRL: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Char('z'), CTRL),
    cmd: Command::Undo,
    help: "undo",
    secondary: false,
};

pub(crate) const REDO_SUP_SHIFT: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Char('z'), SUP_SHIFT),
    cmd: Command::Redo,
    help: "redo",
    secondary: false,
};

pub(crate) const REDO_SUP_SHIFT_ALT: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Char('Z'), SUP),
    cmd: Command::Redo,
    help: "redo",
    secondary: true,
};

pub(crate) const REDO_CTRL_Y: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Char('y'), CTRL),
    cmd: Command::Redo,
    help: "redo",
    secondary: false,
};

pub(crate) const DELETE_LINE: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Char('k'), SUP_SHIFT),
    cmd: Command::DeleteLine,
    help: "delete line",
    secondary: false,
};

pub(crate) const DELETE_LINE_ALT: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Char('K'), SUP),
    cmd: Command::DeleteLine,
    help: "delete line",
    secondary: true,
};

pub(crate) const SAVE: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Char('s'), SUP),
    cmd: Command::Save,
    help: "save",
    secondary: false,
};

pub(crate) const RELOAD: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Char('r'), SUP),
    cmd: Command::Reload,
    help: "reload graphics",
    secondary: false,
};
