//! `EDITOR_BINDINGS` — the ONE source of truth for every editor-pane chord
//! (plan WP6.S1/S7, WP10.S3). `keymap::resolve` no longer hand-matches
//! these chords itself; it delegates straight to `resolve_in(EDITOR_
//! BINDINGS, key)`, so a chord either has a row here or it does not
//! resolve — `help.rs`'s reflection pass and the startup collision index
//! (`index::validate`) read the exact same data the live dispatch path
//! uses, closing the recorded exception `help.rs` used to carry (a hand-
//! maintained editor key list, kept in sync by hand — exactly the kind of
//! parallel source of truth that must never exist) AND the drift a second, hand-written match
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
//!
//! The 59 rows themselves are split across four sibling modules by natural
//! group — `motion` (plain cursor movement, viewport-only scroll, link
//! follow), `selection` (chords that extend or create a selection),
//! `editing` (chords that mutate the document, move/clone a line, add a
//! multi-cursor, undo/redo, save), `clipboard` (copy/cut/paste) — to bring
//! this file under the 500-line budget. `EDITOR_BINDINGS` below lists
//! every row by name in the EXACT original order (resolution is order-
//! sensitive), regardless of which sibling module defines it.

mod clipboard;
mod editing;
mod motion;
// Re-exported so a caller outside this module (`help.rs`'s reload test)
// can name the reload binding by the same constant `EDITOR_BINDINGS`
// itself lists, rather than a hardcoded help/key-label string.
pub(crate) use editing::RELOAD;
mod selection;

use crate::binding::Binding;
use crate::keymap::{Command, Mods};

pub(crate) const NONE: Mods = Mods::NONE;
pub(crate) const SHIFT: Mods = Mods {
    shift: true,
    alt: false,
    ctrl: false,
    sup: false,
};
pub(crate) const ALT: Mods = Mods {
    shift: false,
    alt: true,
    ctrl: false,
    sup: false,
};
pub(crate) const SHIFT_ALT: Mods = Mods {
    shift: true,
    alt: true,
    ctrl: false,
    sup: false,
};
pub(crate) const CTRL: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: true,
    sup: false,
};
pub(crate) const CTRL_SHIFT: Mods = Mods {
    shift: true,
    alt: false,
    ctrl: true,
    sup: false,
};
pub(crate) const SUP: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: false,
    sup: true,
};
pub(crate) const SUP_SHIFT: Mods = Mods {
    shift: true,
    alt: false,
    ctrl: false,
    sup: true,
};
pub(crate) const ALT_SUP: Mods = Mods {
    shift: false,
    alt: true,
    ctrl: false,
    sup: true,
};

/// One row per chord `keymap::resolve` binds (Save/quit chords excepted —
/// see this module's doc comment).
pub const EDITOR_BINDINGS: &[Binding<Command>] = &[
    motion::CHAR_LEFT,
    motion::CHAR_RIGHT,
    selection::SELECT_CHAR_LEFT,
    selection::SELECT_CHAR_RIGHT,
    motion::WORD_LEFT_ARROW,
    motion::WORD_RIGHT_ARROW,
    selection::SELECT_WORD_LEFT_ARROW,
    selection::SELECT_WORD_RIGHT_ARROW,
    selection::SELECT_WORD_LEFT_B,
    selection::SELECT_WORD_RIGHT_F,
    motion::LINE_UP,
    motion::LINE_DOWN,
    selection::SELECT_LINE_UP,
    selection::SELECT_LINE_DOWN,
    editing::MOVE_LINE_UP,
    editing::MOVE_LINE_DOWN,
    editing::CLONE_LINE_UP,
    editing::CLONE_LINE_DOWN,
    editing::ADD_CURSOR_ABOVE,
    editing::ADD_CURSOR_BELOW,
    motion::LINE_START,
    motion::LINE_END,
    selection::SELECT_LINE_START,
    selection::SELECT_LINE_END,
    motion::PAGE_UP,
    motion::PAGE_DOWN,
    selection::SELECT_PAGE_UP,
    selection::SELECT_PAGE_DOWN,
    motion::PAGE_UP_CTRL_U,
    editing::DELETE_LEFT,
    editing::DELETE_WORD_LEFT,
    editing::DELETE_RIGHT,
    editing::DELETE_WORD_RIGHT,
    editing::INDENT,
    editing::OUTDENT_SHIFT_TAB,
    editing::OUTDENT_BACKTAB,
    motion::WORD_LEFT_B,
    motion::WORD_RIGHT_F,
    selection::SELECT_ALL_CTRL,
    selection::SELECT_ALL_SUP,
    clipboard::COPY_SUP,
    clipboard::COPY_CTRL_SHIFT,
    clipboard::CUT,
    clipboard::PASTE,
    editing::UNDO_SUP,
    editing::UNDO_CTRL,
    editing::REDO_SUP_SHIFT,
    editing::REDO_CTRL_Y,
    editing::DELETE_LINE,
    editing::SAVE,
    motion::SCROLL_LINE_UP,
    motion::SCROLL_LINE_DOWN,
    motion::SCROLL_HALF_PAGE_UP,
    motion::SCROLL_HALF_PAGE_DOWN,
    motion::CENTRE_CURSOR,
    motion::CURSOR_TO_TOP,
    motion::CURSOR_TO_BOTTOM,
    motion::FOLLOW_LINK_SUP,
    motion::FOLLOW_LINK_CTRL,
    RELOAD,
];

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::keymap::index;

    #[test]
    fn editor_bindings_have_no_duplicate_key() {
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
        use crate::binding::KeyMatch;
        use crate::keymap::{KeyInput, resolve};

        for binding in EDITOR_BINDINGS {
            let pattern = binding.key;
            let code = match pattern.key {
                KeyMatch::Code(code) => code,
                KeyMatch::Printable => unreachable!("EDITOR_BINDINGS has no wildcard rows"),
            };
            let key = KeyInput {
                code,
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

    /// There is no existing cross-table keymap-union guard to lean on (this
    /// codebase deliberately allows different tables — vim vs. editor,
    /// global vs. editor — to reuse the same physical key), so the reload
    /// chord's own safety net has to be a dedicated test asserting no OTHER
    /// row in THIS table already claims `⌘R`. `index::validate` (exercised
    /// above) already rejects two rows sharing an identical key outright —
    /// this test asserts that fact specifically for the reload binding, by
    /// identity.
    #[test]
    fn reload_key_is_not_already_bound_elsewhere_in_the_editor_table() {
        let claimants: Vec<&'static str> = EDITOR_BINDINGS
            .iter()
            .filter(|b| b.key == editing::RELOAD.key)
            .map(|b| b.help)
            .collect();
        assert_eq!(
            claimants,
            vec![editing::RELOAD.help],
            "⌘R must be claimed by exactly the reload binding, not shared with any other row"
        );
    }
}
