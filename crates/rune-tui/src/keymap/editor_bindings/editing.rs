//! Editing group of `EDITOR_BINDINGS` (chords that mutate the document,
//! move/clone a line, add a multi-cursor, undo/redo, or save). Split out
//! of `editor_bindings.rs` to bring that file under the §1.6 500-line
//! budget; assembled back into the single `EDITOR_BINDINGS` table (see
//! that module's doc comment) in the exact original order — this module
//! only owns the definitions.

use crate::binding::{Binding, KeyPattern};
use crate::keymap::{Command, KeyCode};

use super::{ALT, ALT_SUP, CTRL, NONE, SHIFT, SHIFT_ALT, SUP, SUP_SHIFT};

pub(crate) const MOVE_LINE_UP: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Up, ALT)],
    cmd: Command::MoveLineUp,
    help: "move line up",
    when: "",
    alias: false,
};

pub(crate) const MOVE_LINE_DOWN: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Down, ALT)],
    cmd: Command::MoveLineDown,
    help: "move line down",
    when: "",
    alias: false,
};

pub(crate) const CLONE_LINE_UP: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Up, SHIFT_ALT)],
    cmd: Command::CloneLineUp,
    help: "clone line up",
    when: "",
    alias: false,
};

pub(crate) const CLONE_LINE_DOWN: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Down, SHIFT_ALT)],
    cmd: Command::CloneLineDown,
    help: "clone line down",
    when: "",
    alias: false,
};

pub(crate) const ADD_CURSOR_ABOVE: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Up, ALT_SUP)],
    cmd: Command::AddCursorAbove,
    help: "cursor above",
    when: "",
    alias: false,
};

pub(crate) const ADD_CURSOR_BELOW: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Down, ALT_SUP)],
    cmd: Command::AddCursorBelow,
    help: "cursor below",
    when: "",
    alias: false,
};

pub(crate) const DELETE_LEFT: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Backspace, NONE)],
    cmd: Command::DeleteLeft,
    help: "delete left",
    when: "",
    alias: false,
};

pub(crate) const DELETE_WORD_LEFT: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Backspace, ALT)],
    cmd: Command::DeleteWordLeft,
    help: "delete word left",
    when: "",
    alias: false,
};

pub(crate) const DELETE_RIGHT: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Delete, NONE)],
    cmd: Command::DeleteRight,
    help: "delete right",
    when: "",
    alias: false,
};

pub(crate) const DELETE_WORD_RIGHT: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Delete, ALT)],
    cmd: Command::DeleteWordRight,
    help: "delete word right",
    when: "",
    alias: false,
};

pub(crate) const INDENT: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Tab, NONE)],
    cmd: Command::Indent,
    help: "indent",
    when: "",
    alias: false,
};

pub(crate) const OUTDENT_SHIFT_TAB: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Tab, SHIFT)],
    cmd: Command::Outdent,
    help: "outdent",
    when: "",
    alias: false,
};

pub(crate) const OUTDENT_BACKTAB: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::BackTab, NONE)],
    cmd: Command::Outdent,
    help: "outdent",
    when: "",
    alias: false,
};

pub(crate) const UNDO_SUP: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Char('z'), SUP)],
    cmd: Command::Undo,
    help: "undo",
    when: "",
    alias: false,
};

pub(crate) const UNDO_CTRL: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Char('z'), CTRL)],
    cmd: Command::Undo,
    help: "undo",
    when: "",
    alias: false,
};

pub(crate) const REDO_SUP_SHIFT: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Char('z'), SUP_SHIFT)],
    cmd: Command::Redo,
    help: "redo",
    when: "",
    alias: false,
};

pub(crate) const REDO_CTRL_Y: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Char('y'), CTRL)],
    cmd: Command::Redo,
    help: "redo",
    when: "",
    alias: false,
};

pub(crate) const DELETE_LINE: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Char('k'), SUP_SHIFT)],
    cmd: Command::DeleteLine,
    help: "delete line",
    when: "",
    alias: false,
};

pub(crate) const SAVE: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Char('s'), SUP)],
    cmd: Command::Save,
    help: "save",
    when: "",
    alias: false,
};

/// Plan WP6.S1/S2 — re-decode and retransmit an image document. `⌘R` is
/// unused anywhere else in this table (see `editor_bindings.rs`'s own
/// collision test); gated on the `image` `when` atom so a non-image
/// document's help/footer surfaces never advertise a chord that would do
/// nothing there.
pub(crate) const RELOAD: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Char('r'), SUP)],
    cmd: Command::Reload,
    help: "reload image",
    when: "image",
    alias: false,
};
