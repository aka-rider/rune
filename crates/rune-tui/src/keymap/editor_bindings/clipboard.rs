use crate::binding::{Binding, KeyPattern};
use crate::keymap::{Command, KeyCode};

use super::{CTRL, CTRL_SHIFT, SUP};

pub(crate) const COPY_SUP: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Char('c'), SUP),
    cmd: Command::Copy,
    help: "copy",
    secondary: false,
};

pub(crate) const COPY_CTRL_SHIFT: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Char('c'), CTRL_SHIFT),
    cmd: Command::Copy,
    help: "copy",
    secondary: false,
};

pub(crate) const COPY_CTRL_SHIFT_ALT: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Char('C'), CTRL),
    cmd: Command::Copy,
    help: "copy",
    secondary: true,
};

pub(crate) const CUT: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Char('x'), SUP),
    cmd: Command::Cut,
    help: "cut",
    secondary: false,
};

pub(crate) const PASTE: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Char('v'), SUP),
    cmd: Command::Paste,
    help: "paste",
    secondary: false,
};
