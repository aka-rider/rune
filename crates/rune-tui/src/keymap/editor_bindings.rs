// The quit chords are deliberately omitted here: `QuitKey::from_key` claims
// them before `resolve_in` is ever consulted, so `ctrl+d` is not a
// `PageDown` row here — only `ctrl+u` reaches `Command::PageUp` from a
// `Char` chord.
//
// `Save`'s exact `sup`-only chord does have a row here even though
// `global::GLOBAL_BINDINGS` already resolves the identical chord one stage
// earlier: `keymap::resolve` is also called directly by callers that never
// go through that layered pipeline (`rune-fuzz`'s driver tags each
// keystroke with `keymap::resolve(key)` for its save-inflight invariant),
// and those callers need the exact chord to identify as `Command::Save`.
// Only the exact chord gets a row — no shift/alt variant.

mod clipboard;
mod editing;
mod motion;
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

pub const EDITOR_BINDINGS: &[Binding<Command>] = &[
    motion::CHAR_LEFT,
    motion::CHAR_RIGHT,
    selection::SELECT_CHAR_LEFT,
    selection::SELECT_CHAR_RIGHT,
    motion::WORD_LEFT_ARROW,
    motion::WORD_RIGHT_ARROW,
    selection::SELECT_WORD_LEFT_ARROW,
    selection::SELECT_WORD_RIGHT_ARROW,
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
    motion::MATCH_BRACKET,
    selection::SELECT_MATCH_BRACKET,
    selection::SELECT_MATCH_BRACKET_PIPE,
    selection::SELECT_ALL_CTRL,
    selection::SELECT_ALL_SUP,
    clipboard::COPY_SUP,
    clipboard::COPY_CTRL_SHIFT,
    clipboard::COPY_CTRL_SHIFT_ALT,
    clipboard::CUT,
    clipboard::PASTE,
    editing::UNDO_SUP,
    editing::UNDO_CTRL,
    editing::REDO_SUP_SHIFT,
    editing::REDO_SUP_SHIFT_ALT,
    editing::REDO_CTRL_Y,
    editing::DELETE_LINE,
    editing::DELETE_LINE_ALT,
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

    // `index::validate` already rejects any two rows in this table sharing
    // a key; this test pins that fact specifically for the reload binding,
    // by identity, since different tables are allowed to reuse a physical
    // key and only a same-table collision is ever invalid.
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

    #[test]
    fn alternate_key_forms_exist_for_shifted_chars_under_kitty_protocol() {
        use crate::binding::resolve_in;
        use crate::keymap::{KeyCode, KeyInput, Mods};

        struct AffectedAction {
            lowercase_shift: (KeyCode, Mods),
            uppercase_noshift: (KeyCode, Mods),
            help: &'static str,
        }

        let affected = [
            AffectedAction {
                lowercase_shift: (KeyCode::Char('z'), super::SUP_SHIFT),
                uppercase_noshift: (KeyCode::Char('Z'), super::SUP),
                help: "redo",
            },
            AffectedAction {
                lowercase_shift: (KeyCode::Char('k'), super::SUP_SHIFT),
                uppercase_noshift: (KeyCode::Char('K'), super::SUP),
                help: "delete line",
            },
            AffectedAction {
                lowercase_shift: (KeyCode::Char('c'), super::CTRL_SHIFT),
                uppercase_noshift: (KeyCode::Char('C'), super::CTRL),
                help: "copy",
            },
        ];

        for action in affected {
            let lowercase_shift_key = KeyInput {
                code: action.lowercase_shift.0,
                mods: action.lowercase_shift.1,
            };
            let uppercase_noshift_key = KeyInput {
                code: action.uppercase_noshift.0,
                mods: action.uppercase_noshift.1,
            };

            let lowercase_result = resolve_in(EDITOR_BINDINGS, lowercase_shift_key);
            let uppercase_result = resolve_in(EDITOR_BINDINGS, uppercase_noshift_key);

            assert!(
                lowercase_result.is_some(),
                "action '{}': lowercase shift form {:?} should resolve",
                action.help,
                lowercase_shift_key
            );

            assert!(
                uppercase_result.is_some(),
                "action '{}': uppercase no-shift form {:?} should resolve",
                action.help,
                uppercase_noshift_key
            );

            assert_eq!(
                lowercase_result, uppercase_result,
                "action '{}': both forms should resolve to the same command",
                action.help
            );
        }
    }
}
