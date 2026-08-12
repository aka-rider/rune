//! Selection group of `EDITOR_BINDINGS` (chords that extend or create a
//! selection, plus select-all). Split out of `editor_bindings.rs` to bring
//! that file under the 500-line budget; assembled back into the
//! single `EDITOR_BINDINGS` table (see that module's doc comment) in the
//! exact original order — this module only owns the definitions.

use crate::binding::{Binding, KeyPattern};
use crate::keymap::{Command, Extend, KeyCode, Motion};

use super::{CTRL, SHIFT, SHIFT_ALT, SUP};

pub(crate) const SELECT_CHAR_LEFT: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Left, SHIFT),
    cmd: Command::Motion(Motion::CharLeft, Extend::Yes),
    help: "select char left",
    alias: false,
};

pub(crate) const SELECT_CHAR_RIGHT: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Right, SHIFT),
    cmd: Command::Motion(Motion::CharRight, Extend::Yes),
    help: "select char right",
    alias: false,
};

pub(crate) const SELECT_WORD_LEFT_ARROW: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Left, SHIFT_ALT),
    cmd: Command::Motion(Motion::WordLeft, Extend::Yes),
    help: "select word left",
    alias: false,
};

pub(crate) const SELECT_WORD_RIGHT_ARROW: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Right, SHIFT_ALT),
    cmd: Command::Motion(Motion::WordRight, Extend::Yes),
    help: "select word right",
    alias: false,
};

// The `Char('b')`/`Char('f')` word-motion mirror of the rows above:
// plain ALT is already covered further down (`WordLeft`/`WordRight`),
// these two complete the four-way mirror with the SHIFT+ALT "select"
// variant — previously only reachable through a loose `resolve_char`
// arm that didn't check `shift`, so `⌥⇧B`/`⌥⇧F` silently collapsed a
// selection (moved) instead of extending it.
pub(crate) const SELECT_WORD_LEFT_B: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Char('b'), SHIFT_ALT),
    cmd: Command::Motion(Motion::WordLeft, Extend::Yes),
    help: "select word left",
    alias: false,
};

pub(crate) const SELECT_WORD_RIGHT_F: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Char('f'), SHIFT_ALT),
    cmd: Command::Motion(Motion::WordRight, Extend::Yes),
    help: "select word right",
    alias: false,
};

pub(crate) const SELECT_LINE_UP: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Up, SHIFT),
    cmd: Command::Motion(Motion::LineUp, Extend::Yes),
    help: "select line up",
    alias: false,
};

pub(crate) const SELECT_LINE_DOWN: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Down, SHIFT),
    cmd: Command::Motion(Motion::LineDown, Extend::Yes),
    help: "select line down",
    alias: false,
};

pub(crate) const SELECT_LINE_START: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Home, SHIFT),
    cmd: Command::Motion(Motion::LineStart, Extend::Yes),
    help: "select to line start",
    alias: false,
};

pub(crate) const SELECT_LINE_END: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::End, SHIFT),
    cmd: Command::Motion(Motion::LineEnd, Extend::Yes),
    help: "select to line end",
    alias: false,
};

pub(crate) const SELECT_PAGE_UP: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::PageUp, SHIFT),
    cmd: Command::Motion(Motion::PageUp, Extend::Yes),
    help: "select page up",
    alias: false,
};

pub(crate) const SELECT_PAGE_DOWN: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::PageDown, SHIFT),
    cmd: Command::Motion(Motion::PageDown, Extend::Yes),
    help: "select page down",
    alias: false,
};

pub(crate) const SELECT_ALL_CTRL: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Char('a'), CTRL),
    cmd: Command::SelectAll,
    help: "select all",
    alias: false,
};

pub(crate) const SELECT_ALL_SUP: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Char('a'), SUP),
    cmd: Command::SelectAll,
    help: "select all",
    alias: false,
};
