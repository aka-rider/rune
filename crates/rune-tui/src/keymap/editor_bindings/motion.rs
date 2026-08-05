//! Motion group of `EDITOR_BINDINGS` (plain cursor movement, viewport-only
//! scroll, and link-follow — no selection change, no document mutation).
//! Split out of `editor_bindings.rs` to bring that file under the
//! 500-line budget; assembled back into the single `EDITOR_BINDINGS` table
//! (see that module's doc comment) in the exact original order — this
//! module only owns the definitions.

use crate::binding::{Binding, KeyPattern};
use crate::keymap::{Command, KeyCode};

use super::{ALT, CTRL, NONE, SUP};

pub(crate) const CHAR_LEFT: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Left, NONE)],
    cmd: Command::CharLeft,
    help: "move left",
    when: "",
    alias: false,
};

pub(crate) const CHAR_RIGHT: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Right, NONE)],
    cmd: Command::CharRight,
    help: "move right",
    when: "",
    alias: false,
};

pub(crate) const WORD_LEFT_ARROW: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Left, ALT)],
    cmd: Command::WordLeft,
    help: "word left",
    when: "",
    alias: false,
};

pub(crate) const WORD_RIGHT_ARROW: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Right, ALT)],
    cmd: Command::WordRight,
    help: "word right",
    when: "",
    alias: false,
};

pub(crate) const LINE_UP: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Up, NONE)],
    cmd: Command::LineUp,
    help: "move up",
    when: "",
    alias: false,
};

pub(crate) const LINE_DOWN: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Down, NONE)],
    cmd: Command::LineDown,
    help: "move down",
    when: "",
    alias: false,
};

pub(crate) const LINE_START: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Home, NONE)],
    cmd: Command::LineStart,
    help: "line start",
    when: "",
    alias: false,
};

pub(crate) const LINE_END: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::End, NONE)],
    cmd: Command::LineEnd,
    help: "line end",
    when: "",
    alias: false,
};

pub(crate) const PAGE_UP: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::PageUp, NONE)],
    cmd: Command::PageUp,
    help: "page up",
    when: "",
    alias: false,
};

pub(crate) const PAGE_DOWN: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::PageDown, NONE)],
    cmd: Command::PageDown,
    help: "page down",
    when: "",
    alias: false,
};

pub(crate) const PAGE_UP_CTRL_U: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Char('u'), CTRL)],
    cmd: Command::PageUp,
    help: "page up",
    when: "",
    alias: false,
};

pub(crate) const WORD_LEFT_B: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Char('b'), ALT)],
    cmd: Command::WordLeft,
    help: "word left",
    when: "",
    alias: false,
};

pub(crate) const WORD_RIGHT_F: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Char('f'), ALT)],
    cmd: Command::WordRight,
    help: "word right",
    when: "",
    alias: false,
};

// WP7.S2/S7: viewport-only scroll commands — vim/Helix parity, see
// `keymap::resolve`'s doc comments on each arm for the exact rationale.
pub(crate) const SCROLL_LINE_UP: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Up, CTRL)],
    cmd: Command::ScrollLineUp,
    help: "scroll line up",
    when: "",
    alias: false,
};

pub(crate) const SCROLL_LINE_DOWN: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Down, CTRL)],
    cmd: Command::ScrollLineDown,
    help: "scroll line down",
    when: "",
    alias: false,
};

pub(crate) const SCROLL_HALF_PAGE_UP: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::PageUp, CTRL)],
    cmd: Command::ScrollHalfPageUp,
    help: "scroll half page up",
    when: "",
    alias: false,
};

pub(crate) const SCROLL_HALF_PAGE_DOWN: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::PageDown, CTRL)],
    cmd: Command::ScrollHalfPageDown,
    help: "scroll half page down",
    when: "",
    alias: false,
};

pub(crate) const CENTRE_CURSOR: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Char('l'), CTRL)],
    cmd: Command::CentreCursor,
    help: "centre cursor",
    when: "",
    alias: false,
};

pub(crate) const CURSOR_TO_TOP: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Home, CTRL)],
    cmd: Command::CursorToTop,
    help: "cursor to top of view",
    when: "",
    alias: false,
};

pub(crate) const CURSOR_TO_BOTTOM: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::End, CTRL)],
    cmd: Command::CursorToBottom,
    help: "cursor to bottom of view",
    when: "",
    alias: false,
};

// WP5.S7: follow the link under the cursor — Super or Ctrl held, both
// mirroring the `keymap::resolve` arms exactly.
pub(crate) const FOLLOW_LINK_SUP: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Enter, SUP)],
    cmd: Command::FollowLink,
    help: "follow link",
    when: "",
    alias: false,
};

pub(crate) const FOLLOW_LINK_CTRL: Binding<Command> = Binding {
    keys: &[KeyPattern::new(KeyCode::Enter, CTRL)],
    cmd: Command::FollowLink,
    help: "follow link",
    when: "",
    alias: false,
};
