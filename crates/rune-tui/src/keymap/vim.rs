//! A minimal vim binding set (plan WP6.S8). Its whole point is to
//! demonstrate that collision validation is scoped per binding set
//! (`crate::keymap::index::validate`): `h`/`j`/`k`/`l`/`i` deliberately
//! reuse physical keys `editor_bindings::EDITOR_BINDINGS` ALSO binds (`i`
//! is an ordinary insertable character there), which is fine because only
//! one set is ever active at once (`BindingSet`, a field on `App`
//! defaulting to `BindingSet::Default` — the plan's "VS Code set").
//! `validate` enforces that scoping at the TYPE level here, not just by
//! convention: `VimCommand` and `Command` are different types, so
//! `Binding<VimCommand>` and `Binding<Command>` can never even be compared
//! by the same `validate::<C>` call.
//!
//! Full vim modal editing — mode-dependent live dispatch, `i` actually
//! entering an insert mode `app::handle_editor_key` respects — is
//! explicitly out of scope for this whole plan (see the plan's Goal
//! section: "Explicitly not in this plan: ... full vim modal mode").
//! `VimCommand`/`VIM_BINDINGS` exist to be validated and (once a future WP
//! wires it up) help-documented; `app::handle_editor_key` does not consult
//! this table yet.

use crate::binding::{Binding, KeyPattern};
use crate::keymap::{KeyCode, Mods};

/// Selects which binding set governs the editor pane (plan WP6.S8) — a
/// field on `App`, defaulting to `Default` (the VS Code-style set this
/// crate has had since WP2). Switching to `Vim` only ever changes which
/// table `resolve`-style dispatch consults; it does not by itself imply
/// modal (normal/insert) editing semantics — see this module's doc comment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BindingSet {
    #[default]
    Default,
    Vim,
}

/// The vim set's own command space — deliberately NOT `keymap::Command`:
/// keeping it a separate type is what makes `index::validate` unable to
/// even compare this table against `editor_bindings::EDITOR_BINDINGS`,
/// enforcing "collision validation is per binding set" at compile time
/// rather than by a runtime convention someone could get wrong later.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VimCommand {
    Left,
    Down,
    Up,
    Right,
    /// `i` — see this module's doc comment: not yet wired to any actual
    /// mode change.
    EnterInsert,
}

const NONE: Mods = Mods::NONE;

/// `h`/`j`/`k`/`l`/`i`, all unconditional (`when: ""`) for now — a `mode`-
/// gated version (`when: "mode == \"normal\""`) is the natural next step
/// once a real normal/insert distinction exists to gate on (`when::
/// Context::mode` already carries that field, ready for it).
pub const VIM_BINDINGS: &[Binding<VimCommand>] = &[
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('h'), NONE)],
        cmd: VimCommand::Left,
        help: "left",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('j'), NONE)],
        cmd: VimCommand::Down,
        help: "down",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('k'), NONE)],
        cmd: VimCommand::Up,
        help: "up",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('l'), NONE)],
        cmd: VimCommand::Right,
        help: "right",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('i'), NONE)],
        cmd: VimCommand::EnterInsert,
        help: "insert",
        when: "",
        alias: false,
    },
];

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::keymap::editor_bindings::EDITOR_BINDINGS;
    use crate::keymap::index;

    /// Startup-gate stand-in (plan WP6.S4) — see `global::tests`'s
    /// identical note.
    #[test]
    fn vim_bindings_have_no_prefix_collision() {
        assert!(index::validate(VIM_BINDINGS).is_ok());
    }

    #[test]
    fn default_binding_set_is_the_vs_code_set() {
        assert_eq!(BindingSet::default(), BindingSet::Default);
    }

    #[test]
    fn vim_and_editor_bindings_legitimately_reuse_the_same_physical_keys() {
        // `i` (bare, no modifiers) is `VimCommand::EnterInsert` here and an
        // ordinary insertable character in the default set — proof the two
        // tables are validated ENTIRELY independently, never against each
        // other (different `Binding<C>` instantiations; `index::validate`
        // takes one table at a time).
        let vim_has_bare_i = VIM_BINDINGS
            .iter()
            .any(|b| b.keys == [KeyPattern::new(KeyCode::Char('i'), NONE)]);
        assert!(vim_has_bare_i);
        let editor_binds_bare_i = EDITOR_BINDINGS
            .iter()
            .any(|b| b.keys == [KeyPattern::new(KeyCode::Char('i'), NONE)]);
        assert!(
            !editor_binds_bare_i,
            "editor's own table never binds bare `i` (it falls through to insert-as-text); \
             this asserts the premise this test's name relies on"
        );
        assert!(index::validate(VIM_BINDINGS).is_ok());
        assert!(index::validate(EDITOR_BINDINGS).is_ok());
    }
}
