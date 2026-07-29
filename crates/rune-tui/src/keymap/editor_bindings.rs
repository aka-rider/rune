//! `EDITOR_BINDINGS` — the ONE source of truth for every editor-pane chord
//! (plan WP6.S1/S7, WP10.S3). `keymap::resolve` no longer hand-matches
//! these chords itself; it delegates straight to `resolve_in(EDITOR_
//! BINDINGS, key)`, so a chord either has a row here or it does not
//! resolve — `help.rs`'s reflection pass and the startup collision index
//! (`index::validate`) read the exact same data the live dispatch path
//! uses, closing the recorded exception `help.rs` used to carry (a hand-
//! maintained editor key list, kept in sync by hand, which CONSTITUTION
//! §12 says may not exist) AND the drift a second, hand-written match
//! used to allow (a loose modifier guard here once let `⌘⇧S` fall through
//! to a real save — see the crate's `CODE-REVIEW.md`, rune-tui B finding
//! 3 — since a hand-written `match` arm can check less than its whole
//! `Mods`, while `KeyPattern::matches` never can).
//!
//! The quit chords are deliberately OMITTED here: `QuitKey::from_key`
//! claims them before `resolve_in` is ever consulted (`keymap::resolve`
//! above), so `ctrl+d` is NOT a `PageDown` row here — only `ctrl+u` reaches
//! `Command::PageUp` from a `Char` chord.
//!
//! `Save`'s EXACT `sup`-only chord DOES have a row here, even though
//! `global::GLOBAL_BINDINGS` already resolves the identical chord one
//! stage earlier (`app::handle_key`'s stage 2, before any pane — including
//! the editor's own resolver — ever sees the key): the row below is dead
//! code on that live path, kept anyway because `keymap::resolve` is also
//! called directly by things that never go through the layered
//! `app::handle_key` pipeline (e.g. `rune-fuzz`'s driver tags each
//! keystroke with `keymap::resolve(key)` for its `SAVE-INFLIGHT-SM`
//! invariant) — those callers still need the EXACT chord to identify as
//! `Command::Save`. Only the exact chord: no shift/alt variant gets a row,
//! which is the actual fix (`CODE-REVIEW.md` rune-tui B finding 3) — a
//! loose `resolve_char` arm here once let `⌘⇧S`/`⌘⌥S` through too.

use crate::binding::{Binding, KeyPattern};
use crate::keymap::{Command, KeyCode, Mods};

const NONE: Mods = Mods::NONE;
const SHIFT: Mods = Mods {
    shift: true,
    alt: false,
    ctrl: false,
    sup: false,
};
const ALT: Mods = Mods {
    shift: false,
    alt: true,
    ctrl: false,
    sup: false,
};
const SHIFT_ALT: Mods = Mods {
    shift: true,
    alt: true,
    ctrl: false,
    sup: false,
};
const CTRL: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: true,
    sup: false,
};
const CTRL_SHIFT: Mods = Mods {
    shift: true,
    alt: false,
    ctrl: true,
    sup: false,
};
const SUP: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: false,
    sup: true,
};
const SUP_SHIFT: Mods = Mods {
    shift: true,
    alt: false,
    ctrl: false,
    sup: true,
};
const ALT_SUP: Mods = Mods {
    shift: false,
    alt: true,
    ctrl: false,
    sup: true,
};

/// One row per chord `keymap::resolve` binds (Save/quit chords excepted —
/// see this module's doc comment). Every entry's `when` is `""`
/// (unconditional): none of these chords are context-gated today.
pub const EDITOR_BINDINGS: &[Binding<Command>] = &[
    Binding {
        keys: &[KeyPattern::new(KeyCode::Left, NONE)],
        cmd: Command::CharLeft,
        help: "move left",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Right, NONE)],
        cmd: Command::CharRight,
        help: "move right",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Left, SHIFT)],
        cmd: Command::SelectCharLeft,
        help: "select char left",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Right, SHIFT)],
        cmd: Command::SelectCharRight,
        help: "select char right",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Left, ALT)],
        cmd: Command::WordLeft,
        help: "word left",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Right, ALT)],
        cmd: Command::WordRight,
        help: "word right",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Left, SHIFT_ALT)],
        cmd: Command::SelectWordLeft,
        help: "select word left",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Right, SHIFT_ALT)],
        cmd: Command::SelectWordRight,
        help: "select word right",
        when: "",
        alias: false,
    },
    // The `Char('b')`/`Char('f')` word-motion mirror of the rows above:
    // plain ALT is already covered further down (`WordLeft`/`WordRight`),
    // these two complete the four-way mirror with the SHIFT+ALT "select"
    // variant — previously only reachable through a loose `resolve_char`
    // arm that didn't check `shift`, so `⌥⇧B`/`⌥⇧F` silently collapsed a
    // selection (moved) instead of extending it.
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('b'), SHIFT_ALT)],
        cmd: Command::SelectWordLeft,
        help: "select word left",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('f'), SHIFT_ALT)],
        cmd: Command::SelectWordRight,
        help: "select word right",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Up, NONE)],
        cmd: Command::LineUp,
        help: "move up",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Down, NONE)],
        cmd: Command::LineDown,
        help: "move down",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Up, SHIFT)],
        cmd: Command::SelectLineUp,
        help: "select line up",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Down, SHIFT)],
        cmd: Command::SelectLineDown,
        help: "select line down",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Up, ALT)],
        cmd: Command::MoveLineUp,
        help: "move line up",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Down, ALT)],
        cmd: Command::MoveLineDown,
        help: "move line down",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Up, SHIFT_ALT)],
        cmd: Command::CloneLineUp,
        help: "clone line up",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Down, SHIFT_ALT)],
        cmd: Command::CloneLineDown,
        help: "clone line down",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Up, ALT_SUP)],
        cmd: Command::AddCursorAbove,
        help: "cursor above",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Down, ALT_SUP)],
        cmd: Command::AddCursorBelow,
        help: "cursor below",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Home, NONE)],
        cmd: Command::LineStart,
        help: "line start",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::End, NONE)],
        cmd: Command::LineEnd,
        help: "line end",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Home, SHIFT)],
        cmd: Command::SelectLineStart,
        help: "select to line start",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::End, SHIFT)],
        cmd: Command::SelectLineEnd,
        help: "select to line end",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::PageUp, NONE)],
        cmd: Command::PageUp,
        help: "page up",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::PageDown, NONE)],
        cmd: Command::PageDown,
        help: "page down",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::PageUp, SHIFT)],
        cmd: Command::SelectPageUp,
        help: "select page up",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::PageDown, SHIFT)],
        cmd: Command::SelectPageDown,
        help: "select page down",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('u'), CTRL)],
        cmd: Command::PageUp,
        help: "page up",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Backspace, NONE)],
        cmd: Command::DeleteLeft,
        help: "delete left",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Backspace, ALT)],
        cmd: Command::DeleteWordLeft,
        help: "delete word left",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Delete, NONE)],
        cmd: Command::DeleteRight,
        help: "delete right",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Delete, ALT)],
        cmd: Command::DeleteWordRight,
        help: "delete word right",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Tab, NONE)],
        cmd: Command::Indent,
        help: "indent",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Tab, SHIFT)],
        cmd: Command::Outdent,
        help: "outdent",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::BackTab, NONE)],
        cmd: Command::Outdent,
        help: "outdent",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('b'), ALT)],
        cmd: Command::WordLeft,
        help: "word left",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('f'), ALT)],
        cmd: Command::WordRight,
        help: "word right",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('a'), CTRL)],
        cmd: Command::SelectAll,
        help: "select all",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('a'), SUP)],
        cmd: Command::SelectAll,
        help: "select all",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('c'), SUP)],
        cmd: Command::Copy,
        help: "copy",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('c'), CTRL_SHIFT)],
        cmd: Command::Copy,
        help: "copy",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('x'), SUP)],
        cmd: Command::Cut,
        help: "cut",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('v'), SUP)],
        cmd: Command::Paste,
        help: "paste",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('z'), SUP)],
        cmd: Command::Undo,
        help: "undo",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('z'), CTRL)],
        cmd: Command::Undo,
        help: "undo",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('z'), SUP_SHIFT)],
        cmd: Command::Redo,
        help: "redo",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('y'), CTRL)],
        cmd: Command::Redo,
        help: "redo",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('k'), SUP_SHIFT)],
        cmd: Command::DeleteLine,
        help: "delete line",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('s'), SUP)],
        cmd: Command::Save,
        help: "save",
        when: "",
        alias: false,
    },
    // WP7.S2/S7: viewport-only scroll commands — vim/Helix parity, see
    // `keymap::resolve`'s doc comments on each arm for the exact rationale.
    Binding {
        keys: &[KeyPattern::new(KeyCode::Up, CTRL)],
        cmd: Command::ScrollLineUp,
        help: "scroll line up",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Down, CTRL)],
        cmd: Command::ScrollLineDown,
        help: "scroll line down",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::PageUp, CTRL)],
        cmd: Command::ScrollHalfPageUp,
        help: "scroll half page up",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::PageDown, CTRL)],
        cmd: Command::ScrollHalfPageDown,
        help: "scroll half page down",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('l'), CTRL)],
        cmd: Command::CentreCursor,
        help: "centre cursor",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Home, CTRL)],
        cmd: Command::CursorToTop,
        help: "cursor to top of view",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::End, CTRL)],
        cmd: Command::CursorToBottom,
        help: "cursor to bottom of view",
        when: "",
        alias: false,
    },
    // WP5.S7: follow the link under the cursor — Super or Ctrl held, both
    // mirroring the `keymap::resolve` arms exactly.
    Binding {
        keys: &[KeyPattern::new(KeyCode::Enter, SUP)],
        cmd: Command::FollowLink,
        help: "follow link",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Enter, CTRL)],
        cmd: Command::FollowLink,
        help: "follow link",
        when: "",
        alias: false,
    },
];

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::keymap::index;

    /// Startup-gate stand-in (plan WP6.S4) — see `global::tests`'s
    /// identical note.
    #[test]
    fn editor_bindings_have_no_prefix_collision() {
        assert!(index::validate(EDITOR_BINDINGS).is_ok());
    }

    #[test]
    fn every_row_resolves_through_the_live_dispatch_path() {
        // `resolve` delegates to this table directly (WP10.S3), so this is
        // now near-tautological for the forward direction — kept as a
        // cheap sanity check that a future change to `resolve` doesn't
        // reintroduce a second, hand-written match that drifts from this
        // table. The CONVERSE direction (every chord `resolve` accepts has
        // a row here) is the real anti-drift gate — see the exhaustive
        // sweep test in `keymap.rs`.
        use crate::keymap::{KeyInput, resolve};

        for binding in EDITOR_BINDINGS {
            let pattern = binding.keys.first().expect("every row is single-key here");
            let key = KeyInput {
                code: pattern.code,
                mods: pattern.mods,
            };
            assert_eq!(
                resolve(key),
                Some(binding.cmd),
                "table row {:?} disagrees with keymap::resolve",
                binding.help
            );
        }
    }
}
