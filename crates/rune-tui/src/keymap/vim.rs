// Full vim modal editing is out of scope: nothing consults this table's
// `EnterInsert` yet to actually enter a mode. `VimCommand` is kept as its
// own type, distinct from `keymap::Command`, so a collision check can
// never compare a vim binding against an editor binding — only within one
// binding set, never across two.

use crate::binding::{Binding, KeyPattern};
use crate::keymap::{KeyCode, Mods};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BindingSet {
    #[default]
    Default,
    Vim,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VimCommand {
    Left,
    Down,
    Up,
    Right,
    EnterInsert,
}

const NONE: Mods = Mods::NONE;

pub const VIM_BINDINGS: &[Binding<VimCommand>] = &[
    Binding {
        key: KeyPattern::new(KeyCode::Char('h'), NONE),
        cmd: VimCommand::Left,
        help: "left",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('j'), NONE),
        cmd: VimCommand::Down,
        help: "down",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('k'), NONE),
        cmd: VimCommand::Up,
        help: "up",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('l'), NONE),
        cmd: VimCommand::Right,
        help: "right",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('i'), NONE),
        cmd: VimCommand::EnterInsert,
        help: "insert",
        secondary: false,
    },
];

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::keymap::editor_bindings::EDITOR_BINDINGS;
    use crate::keymap::index;

    #[test]
    fn vim_bindings_have_no_duplicate_key() {
        assert!(index::validate(VIM_BINDINGS).is_ok());
    }

    #[test]
    fn default_binding_set_is_the_vs_code_set() {
        assert_eq!(BindingSet::default(), BindingSet::Default);
    }

    #[test]
    fn vim_and_editor_bindings_legitimately_reuse_the_same_physical_keys() {
        let vim_has_bare_i = VIM_BINDINGS
            .iter()
            .any(|b| b.key == KeyPattern::new(KeyCode::Char('i'), NONE));
        assert!(vim_has_bare_i);
        let editor_binds_bare_i = EDITOR_BINDINGS
            .iter()
            .any(|b| b.key == KeyPattern::new(KeyCode::Char('i'), NONE));
        assert!(
            !editor_binds_bare_i,
            "editor's own table never binds bare `i` (it falls through to insert-as-text); \
             this asserts the premise this test's name relies on"
        );
        assert!(index::validate(VIM_BINDINGS).is_ok());
        assert!(index::validate(EDITOR_BINDINGS).is_ok());
    }
}
