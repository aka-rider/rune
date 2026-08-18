//! The table-validation side of the prefix index: rejects, at startup,
//! two bindings sharing the identical key. Split out of
//! `index.rs` to bring that file under the 500-line budget; `index`
//! re-exports every item here so no import path downstream changed.

use crate::binding::Binding;

/// The one way `validate` can reject a table: two DIFFERENT bindings
/// sharing the exact same key, where first-match-wins (`resolve_in`'s docs)
/// makes the second silently dead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingConflict {
    /// `first`/`second` are the colliding bindings' own `help` labels, in
    /// table order — `second` is the one first-match-wins would silence.
    Duplicate {
        first: &'static str,
        second: &'static str,
    },
}

impl std::fmt::Display for BindingConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BindingConflict::Duplicate { first, second } => write!(
                f,
                "{first:?} and {second:?} bind the identical key — \
                 {second:?} can never fire (first-match-wins)"
            ),
        }
    }
}

impl std::error::Error for BindingConflict {}

/// Rejects, at startup, a table where two bindings share the exact same
/// key: the second is silently dead, undocumented `resolve_in` first-match-
/// wins precedence. Two DIFFERENT tables never collide against each other —
/// call this once per table (each binding-table module's own test does, in
/// lieu of a real process-startup hook this library crate has no place to
/// install; the registry-walking test below covers every table this crate
/// defines in one place, so a new table can no longer ship unvalidated by
/// omission).
pub fn validate<C: Copy>(table: &[Binding<C>]) -> Result<(), BindingConflict> {
    for (i, a) in table.iter().enumerate() {
        for b in table.iter().skip(i + 1) {
            if a.key == b.key {
                return Err(BindingConflict::Duplicate {
                    first: a.help,
                    second: b.help,
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::binding::KeyPattern;
    use crate::keymap::{KeyCode, Mods};

    const CTRL: Mods = Mods {
        shift: false,
        alt: false,
        ctrl: true,
        sup: false,
    };

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum TestCmd {
        Standalone,
    }

    const fn ctrl_k() -> KeyPattern {
        KeyPattern::new(KeyCode::Char('k'), CTRL)
    }

    #[test]
    fn validate_rejects_duplicate_keys() {
        const TABLE: &[Binding<TestCmd>] = &[
            Binding {
                key: ctrl_k(),
                cmd: TestCmd::Standalone,
                help: "first",
                secondary: false,
            },
            Binding {
                key: ctrl_k(),
                cmd: TestCmd::Standalone,
                help: "second",
                secondary: false,
            },
        ];
        let err = validate(TABLE).expect_err("must reject the duplicate key");
        match &err {
            BindingConflict::Duplicate { first, second } => {
                assert_eq!(*first, "first");
                assert_eq!(*second, "second");
            }
        }
    }

    /// `validate` was only ever called by
    /// each table's own hand-written test — `global::GLOBAL_BINDINGS`,
    /// `opentabs::TABS_BINDINGS`, and `explorer_keys::EXPLORER_BINDINGS`
    /// shipped with none, so a shadowed chord in any of them could never
    /// fire and nothing would catch it. One registry-walking test, listing
    /// every binding table this crate defines, closes that gap
    /// structurally: a new table now has to be added HERE to be validated
    /// at all, so its absence is conspicuous rather than silent.
    #[test]
    fn every_registered_binding_table_validates() {
        assert!(validate(crate::global::GLOBAL_BINDINGS).is_ok());
        assert!(validate(crate::keymap::editor_bindings::EDITOR_BINDINGS).is_ok());
        assert!(validate(crate::keymap::vim::VIM_BINDINGS).is_ok());
        assert!(validate(crate::opentabs::TABS_BINDINGS).is_ok());
        assert!(validate(crate::explorer_keys::EXPLORER_BINDINGS).is_ok());
        assert!(validate(crate::explorer_search::EXPLORER_SEARCH_BINDINGS).is_ok());
        assert!(validate(crate::filesearch::keys::FILESEARCH_BINDINGS).is_ok());
        assert!(validate(crate::diff_view::keys::DIFF_BINDINGS).is_ok());
    }

    #[test]
    fn validate_allows_two_bindings_with_different_keys() {
        const TABLE: &[Binding<TestCmd>] = &[
            Binding {
                key: KeyPattern::new(KeyCode::Char('a'), Mods::NONE),
                cmd: TestCmd::Standalone,
                help: "a",
                secondary: false,
            },
            Binding {
                key: KeyPattern::new(KeyCode::Char('b'), Mods::NONE),
                cmd: TestCmd::Standalone,
                help: "b",
                secondary: false,
            },
        ];
        assert!(validate(TABLE).is_ok());
    }
}
