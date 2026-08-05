//! Selection group of `EDITOR_BINDINGS` (chords that extend or create a
//! selection, plus select-all). Split out of `editor_bindings.rs` to bring
//! that file under the 500-line budget; assembled back into the
//! single `EDITOR_BINDINGS` table (see that module's doc comment) in the
//! exact original order — this module only owns the definitions.

use crate::binding::{Binding, KeyPattern};
use crate::keymap::{Command, KeyCode};

use super::{CTRL, SHIFT, SHIFT_ALT, SUP};

pub(crate) const SELECT_CHAR_LEFT: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Left, SHIFT)],
    cmd: Command::SelectCharLeft,
    help: "select char left",
    when: "",
    alias: false,
};

pub(crate) const SELECT_CHAR_RIGHT: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Right, SHIFT)],
    cmd: Command::SelectCharRight,
    help: "select char right",
    when: "",
    alias: false,
};

pub(crate) const SELECT_WORD_LEFT_ARROW: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Left, SHIFT_ALT)],
    cmd: Command::SelectWordLeft,
    help: "select word left",
    when: "",
    alias: false,
};

pub(crate) const SELECT_WORD_RIGHT_ARROW: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Right, SHIFT_ALT)],
    cmd: Command::SelectWordRight,
    help: "select word right",
    when: "",
    alias: false,
};

// The `Char('b')`/`Char('f')` word-motion mirror of the rows above:
// plain ALT is already covered further down (`WordLeft`/`WordRight`),
// these two complete the four-way mirror with the SHIFT+ALT "select"
// variant — previously only reachable through a loose `resolve_char`
// arm that didn't check `shift`, so `⌥⇧B`/`⌥⇧F` silently collapsed a
// selection (moved) instead of extending it.
pub(crate) const SELECT_WORD_LEFT_B: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Char('b'), SHIFT_ALT)],
    cmd: Command::SelectWordLeft,
    help: "select word left",
    when: "",
    alias: false,
};

pub(crate) const SELECT_WORD_RIGHT_F: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Char('f'), SHIFT_ALT)],
    cmd: Command::SelectWordRight,
    help: "select word right",
    when: "",
    alias: false,
};

pub(crate) const SELECT_LINE_UP: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Up, SHIFT)],
    cmd: Command::SelectLineUp,
    help: "select line up",
    when: "",
    alias: false,
};

pub(crate) const SELECT_LINE_DOWN: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Down, SHIFT)],
    cmd: Command::SelectLineDown,
    help: "select line down",
    when: "",
    alias: false,
};

pub(crate) const SELECT_LINE_START: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Home, SHIFT)],
    cmd: Command::SelectLineStart,
    help: "select to line start",
    when: "",
    alias: false,
};

pub(crate) const SELECT_LINE_END: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::End, SHIFT)],
    cmd: Command::SelectLineEnd,
    help: "select to line end",
    when: "",
    alias: false,
};

pub(crate) const SELECT_PAGE_UP: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::PageUp, SHIFT)],
    cmd: Command::SelectPageUp,
    help: "select page up",
    when: "",
    alias: false,
};

pub(crate) const SELECT_PAGE_DOWN: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::PageDown, SHIFT)],
    cmd: Command::SelectPageDown,
    help: "select page down",
    when: "",
    alias: false,
};

pub(crate) const SELECT_ALL_CTRL: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Char('a'), CTRL)],
    cmd: Command::SelectAll,
    help: "select all",
    when: "",
    alias: false,
};

pub(crate) const SELECT_ALL_SUP: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Char('a'), SUP)],
    cmd: Command::SelectAll,
    help: "select all",
    when: "",
    alias: false,
};
