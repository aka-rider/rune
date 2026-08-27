use crate::binding::Binding;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingConflict {
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

// There is no process-startup hook this library crate can install itself,
// so nothing calls this automatically — each binding-table module's own
// test must call it, and never across two different tables (which are
// allowed to collide with each other).
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

    // Every binding table this crate defines is listed here, so a new
    // table has to be added to this list to be validated at all — its
    // absence is conspicuous rather than silent.
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
