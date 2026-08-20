//! Motion group of `EDITOR_BINDINGS` (plain cursor movement, viewport-only
//! scroll, and link-follow — no selection change, no document mutation).
//! Split out of `editor_bindings.rs` to bring that file under the
//! 500-line budget; assembled back into the single `EDITOR_BINDINGS` table
//! (see that module's doc comment) in the exact original order — this
//! module only owns the definitions.

use crate::binding::{Binding, KeyPattern};
use crate::keymap::{Command, Extend, KeyCode, Motion};

use super::{ALT, CTRL, NONE, SUP};

pub(crate) const CHAR_LEFT: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Left, NONE),
    cmd: Command::Motion(Motion::CharLeft, Extend::No),
    help: "move left",
    secondary: false,
};

pub(crate) const CHAR_RIGHT: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Right, NONE),
    cmd: Command::Motion(Motion::CharRight, Extend::No),
    help: "move right",
    secondary: false,
};

pub(crate) const WORD_LEFT_ARROW: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Left, ALT),
    cmd: Command::Motion(Motion::WordLeft, Extend::No),
    help: "word left",
    secondary: false,
};

pub(crate) const WORD_RIGHT_ARROW: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Right, ALT),
    cmd: Command::Motion(Motion::WordRight, Extend::No),
    help: "word right",
    secondary: false,
};

pub(crate) const LINE_UP: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Up, NONE),
    cmd: Command::Motion(Motion::LineUp, Extend::No),
    help: "move up",
    secondary: false,
};

pub(crate) const LINE_DOWN: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Down, NONE),
    cmd: Command::Motion(Motion::LineDown, Extend::No),
    help: "move down",
    secondary: false,
};

pub(crate) const LINE_START: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Home, NONE),
    cmd: Command::Motion(Motion::LineStart, Extend::No),
    help: "line start",
    secondary: false,
};

pub(crate) const LINE_END: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::End, NONE),
    cmd: Command::Motion(Motion::LineEnd, Extend::No),
    help: "line end",
    secondary: false,
};

pub(crate) const PAGE_UP: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::PageUp, NONE),
    cmd: Command::Motion(Motion::PageUp, Extend::No),
    help: "page up",
    secondary: false,
};

pub(crate) const PAGE_DOWN: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::PageDown, NONE),
    cmd: Command::Motion(Motion::PageDown, Extend::No),
    help: "page down",
    secondary: false,
};

pub(crate) const PAGE_UP_CTRL_U: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Char('u'), CTRL),
    cmd: Command::Motion(Motion::PageUp, Extend::No),
    help: "page up",
    secondary: false,
};

pub(crate) const WORD_LEFT_B: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Char('b'), ALT),
    cmd: Command::Motion(Motion::WordLeft, Extend::No),
    help: "word left",
    secondary: false,
};

pub(crate) const WORD_RIGHT_F: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Char('f'), ALT),
    cmd: Command::Motion(Motion::WordRight, Extend::No),
    help: "word right",
    secondary: false,
};

pub(crate) const MATCH_BRACKET_M: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Char('m'), ALT),
    cmd: Command::Motion(Motion::MatchBracket, Extend::No),
    help: "jump to matching bracket",
    secondary: false,
};

// Viewport-only scroll commands — vim/Helix parity, see
// `keymap::resolve`'s doc comments on each arm for the exact rationale.
pub(crate) const SCROLL_LINE_UP: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Up, CTRL),
    cmd: Command::ScrollLineUp,
    help: "scroll line up",
    secondary: false,
};

pub(crate) const SCROLL_LINE_DOWN: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Down, CTRL),
    cmd: Command::ScrollLineDown,
    help: "scroll line down",
    secondary: false,
};

pub(crate) const SCROLL_HALF_PAGE_UP: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::PageUp, CTRL),
    cmd: Command::ScrollHalfPageUp,
    help: "scroll half page up",
    secondary: false,
};

pub(crate) const SCROLL_HALF_PAGE_DOWN: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::PageDown, CTRL),
    cmd: Command::ScrollHalfPageDown,
    help: "scroll half page down",
    secondary: false,
};

pub(crate) const CENTRE_CURSOR: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Char('l'), CTRL),
    cmd: Command::CentreCursor,
    help: "centre cursor",
    secondary: false,
};

pub(crate) const CURSOR_TO_TOP: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Home, CTRL),
    cmd: Command::CursorToTop,
    help: "cursor to top of view",
    secondary: false,
};

pub(crate) const CURSOR_TO_BOTTOM: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::End, CTRL),
    cmd: Command::CursorToBottom,
    help: "cursor to bottom of view",
    secondary: false,
};

// Follow the link under the cursor — Super or Ctrl held, both
// mirroring the `keymap::resolve` arms exactly.
pub(crate) const FOLLOW_LINK_SUP: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Enter, SUP),
    cmd: Command::FollowLink,
    help: "follow link",
    secondary: false,
};

pub(crate) const FOLLOW_LINK_CTRL: Binding<Command> = Binding {
    key: KeyPattern::new(KeyCode::Enter, CTRL),
    cmd: Command::FollowLink,
    help: "follow link",
    secondary: false,
};
