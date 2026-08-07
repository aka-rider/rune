//! Clipboard group of `EDITOR_BINDINGS` (copy/cut/paste). Split out of
//! `editor_bindings.rs` to bring that file under the 500-line budget;
//! assembled back into the single `EDITOR_BINDINGS` table (see that
//! module's doc comment) in the exact original order — this module only
//! owns the definitions.

use crate::binding::{Binding, KeyPattern};
use crate::keymap::{Command, KeyCode};

use super::{CTRL_SHIFT, SUP};

pub(crate) const COPY_SUP: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Char('c'), SUP),
    cmd: Command::Copy,
    help: "copy",
    alias: false,
};

pub(crate) const COPY_CTRL_SHIFT: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Char('c'), CTRL_SHIFT),
    cmd: Command::Copy,
    help: "copy",
    alias: false,
};

pub(crate) const CUT: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Char('x'), SUP),
    cmd: Command::Cut,
    help: "cut",
    alias: false,
};

pub(crate) const PASTE: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Char('v'), SUP),
    cmd: Command::Paste,
    help: "paste",
    alias: false,
};
