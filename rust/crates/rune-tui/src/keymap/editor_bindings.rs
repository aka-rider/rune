//! `EDITOR_BINDINGS` — a tabled MIRROR of `keymap::resolve`'s hand-written
//! match (plan WP6.S1/S7). `resolve` stays the live dispatch path
//! (`app::handle_editor_key` calls it directly, never this table); this
//! table exists purely so `help.rs`'s reflection pass and the startup
//! collision index (`index::validate`) have real data to walk, closing the
//! recorded exception `help.rs` used to carry — a hand-maintained editor
//! key list, kept in sync by hand, which CONSTITUTION §12 says may not
//! exist.
//!
//! `Save` and the quit chords are deliberately OMITTED here, exactly as the
//! hand-written section it replaces omitted them: they already have their
//! own `## Global` rows (`global::GLOBAL_BINDINGS`) — stage 2 of
//! `app::handle_key`'s pipeline resolves them before the editor's own
//! resolver ever sees them. In particular `ctrl+d` is NOT a `PageDown` row
//! here: on this binding table `QuitKey::from_key` claims plain `ctrl+d` as
//! the second quit chord, so `keymap::resolve_char` has no `'d'` arm at all
//! — only `ctrl+u` reaches `Command::PageUp` from a `Char` chord.

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

/// One row per chord `keymap::resolve` binds (Save/quit chords excepted —
/// see this module's doc comment). Every entry's `when` is `""`
/// (unconditional): none of these chords are context-gated today.
pub const EDITOR_BINDINGS: &[Binding<Command>] = &[
    Binding {
        keys: &[KeyPattern::new(KeyCode::Left, NONE)],
        cmd: Command::CharLeft,
        help: "move left",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Right, NONE)],
        cmd: Command::CharRight,
        help: "move right",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Left, SHIFT)],
        cmd: Command::SelectCharLeft,
        help: "select char left",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Right, SHIFT)],
        cmd: Command::SelectCharRight,
        help: "select char right",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Left, ALT)],
        cmd: Command::WordLeft,
        help: "word left",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Right, ALT)],
        cmd: Command::WordRight,
        help: "word right",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Left, SHIFT_ALT)],
        cmd: Command::SelectWordLeft,
        help: "select word left",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Right, SHIFT_ALT)],
        cmd: Command::SelectWordRight,
        help: "select word right",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Up, NONE)],
        cmd: Command::LineUp,
        help: "move up",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Down, NONE)],
        cmd: Command::LineDown,
        help: "move down",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Up, SHIFT)],
        cmd: Command::SelectLineUp,
        help: "select line up",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Down, SHIFT)],
        cmd: Command::SelectLineDown,
        help: "select line down",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Home, NONE)],
        cmd: Command::LineStart,
        help: "line start",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::End, NONE)],
        cmd: Command::LineEnd,
        help: "line end",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Home, SHIFT)],
        cmd: Command::SelectLineStart,
        help: "select to line start",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::End, SHIFT)],
        cmd: Command::SelectLineEnd,
        help: "select to line end",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::PageUp, NONE)],
        cmd: Command::PageUp,
        help: "page up",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::PageDown, NONE)],
        cmd: Command::PageDown,
        help: "page down",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::PageUp, SHIFT)],
        cmd: Command::SelectPageUp,
        help: "select page up",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::PageDown, SHIFT)],
        cmd: Command::SelectPageDown,
        help: "select page down",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('u'), CTRL)],
        cmd: Command::PageUp,
        help: "page up",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Backspace, NONE)],
        cmd: Command::DeleteLeft,
        help: "delete left",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Delete, NONE)],
        cmd: Command::DeleteRight,
        help: "delete right",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Tab, NONE)],
        cmd: Command::Indent,
        help: "indent",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Tab, SHIFT)],
        cmd: Command::Outdent,
        help: "outdent",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::BackTab, NONE)],
        cmd: Command::Outdent,
        help: "outdent",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('b'), ALT)],
        cmd: Command::WordLeft,
        help: "word left",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('f'), ALT)],
        cmd: Command::WordRight,
        help: "word right",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('a'), CTRL)],
        cmd: Command::SelectAll,
        help: "select all",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('a'), SUP)],
        cmd: Command::SelectAll,
        help: "select all",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('c'), SUP)],
        cmd: Command::Copy,
        help: "copy",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('c'), CTRL_SHIFT)],
        cmd: Command::Copy,
        help: "copy",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('x'), SUP)],
        cmd: Command::Cut,
        help: "cut",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('v'), SUP)],
        cmd: Command::Paste,
        help: "paste",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('z'), SUP)],
        cmd: Command::Undo,
        help: "undo",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('z'), CTRL)],
        cmd: Command::Undo,
        help: "undo",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('z'), SUP_SHIFT)],
        cmd: Command::Redo,
        help: "redo",
        when: "",
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('y'), CTRL)],
        cmd: Command::Redo,
        help: "redo",
        when: "",
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
    fn every_row_resolves_through_the_hand_written_matcher_too() {
        // Tabling `resolve`'s chords must never silently drift from
        // `resolve` itself — each row's single key, fed through `resolve`
        // directly, must produce that same row's `cmd`.
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
