//! The table-validation side of the prefix index (plan WP6.S4, WP10.S4):
//! rejects, at startup, a strict-prefix collision, two bindings sharing the
//! identical sequence, or a `when` clause that fails to parse. Split out of
//! `index.rs` to bring that file under the §1.6 500-line budget; `index`
//! re-exports every item here so no import path downstream changed.

use crate::binding::{Binding, KeyPattern};

/// Names the two colliding bindings by their `help` label — the identity a
/// human debugging a validation failure actually wants, without requiring
/// `C: std::fmt::Debug`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefixCollision {
    pub shorter: &'static str,
    pub longer: &'static str,
}

impl std::fmt::Display for PrefixCollision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:?} is a strict prefix of {:?} within the same binding set",
            self.shorter, self.longer
        )
    }
}

impl std::error::Error for PrefixCollision {}

/// Every way `validate` can reject a table (plan WP10.S4): a strict-prefix
/// collision (see `PrefixCollision`); two DIFFERENT bindings sharing the
/// exact same key sequence, where first-match-wins (`resolve_in`'s docs)
/// makes the second silently dead; or a `when` clause that fails to parse,
/// which — left uncaught — would make that binding permanently inert with
/// nothing to catch it (`crate::when::evaluate_cached`'s `Err` path is
/// treated as "never matches", the same as a clause that legitimately
/// evaluates false).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingConflict {
    Prefix(PrefixCollision),
    /// `first`/`second` are the colliding bindings' own `help` labels, in
    /// table order — `second` is the one first-match-wins would silence.
    Duplicate {
        first: &'static str,
        second: &'static str,
    },
    MalformedWhen {
        label: &'static str,
        err: crate::when::ParseError,
    },
}

impl std::fmt::Display for BindingConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BindingConflict::Prefix(p) => p.fmt(f),
            BindingConflict::Duplicate { first, second } => write!(
                f,
                "{first:?} and {second:?} bind the identical key sequence — \
                 {second:?} can never fire (first-match-wins)"
            ),
            BindingConflict::MalformedWhen { label, err } => {
                write!(f, "{label:?}'s `when` clause fails to parse: {err}")
            }
        }
    }
}

impl std::error::Error for BindingConflict {}

/// Rejects, at startup, three ways a binding table can be self-inconsistent
/// (plan WP6.S4, WP10.S4): a strict-prefix collision (e.g. `["ctrl+k"]`
/// can never coexist with `["ctrl+k", "ctrl+c"]` in the same table — the
/// moment `ctrl+k` alone is pressed there is no way to tell "fire the
/// standalone binding" from "wait for a possible `ctrl+c`" apart); two
/// bindings sharing the exact same sequence (the second is silently dead,
/// undocumented `resolve_in` first-match-wins precedence); or a `when`
/// clause that fails to parse (caught here, at validation time, rather
/// than silently going inert at keystroke time — see `BindingConflict`'s
/// docs). Two DIFFERENT tables never collide against each other — call
/// this once per table (each binding-table module's own test does, in
/// lieu of a real process-startup hook this library crate has no place to
/// install; the registry-walking test below covers every table this crate
/// defines in one place, so a new table can no longer ship unvalidated by
/// omission).
pub fn validate<C: Copy>(table: &[Binding<C>]) -> Result<(), BindingConflict> {
    for binding in table {
        if !binding.when.is_empty()
            && let Err(err) = crate::when::parse(binding.when)
        {
            return Err(BindingConflict::MalformedWhen {
                label: binding.help,
                err,
            });
        }
    }
    for (i, a) in table.iter().enumerate() {
        for b in table.iter().skip(i + 1) {
            if a.keys == b.keys {
                return Err(BindingConflict::Duplicate {
                    first: a.help,
                    second: b.help,
                });
            }
            if is_strict_prefix(a.keys, b.keys) {
                return Err(BindingConflict::Prefix(PrefixCollision {
                    shorter: a.help,
                    longer: b.help,
                }));
            }
            if is_strict_prefix(b.keys, a.keys) {
                return Err(BindingConflict::Prefix(PrefixCollision {
                    shorter: b.help,
                    longer: a.help,
                }));
            }
        }
    }
    Ok(())
}

fn is_strict_prefix(short: &[KeyPattern], long: &[KeyPattern]) -> bool {
    short.len() < long.len() && short.iter().zip(long.iter()).all(|(s, l)| s == l)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
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
        Chord,
    }

    const fn ctrl_k() -> KeyPattern {
        KeyPattern::new(KeyCode::Char('k'), CTRL)
    }
    const fn ctrl_c() -> KeyPattern {
        KeyPattern::new(KeyCode::Char('c'), CTRL)
    }

    #[test]
    fn validate_rejects_a_standalone_binding_that_is_a_prefix_of_a_chord() {
        const TABLE: &[Binding<TestCmd>] = &[
            Binding {
                keys: &[KeyPattern::new(KeyCode::Char('k'), Mods::NONE)],
                cmd: TestCmd::Standalone,
                help: "standalone",
                when: "",
                alias: false,
            },
            Binding {
                keys: &[
                    KeyPattern::new(KeyCode::Char('k'), Mods::NONE),
                    KeyPattern::new(KeyCode::Char('c'), Mods::NONE),
                ],
                cmd: TestCmd::Chord,
                help: "chord",
                when: "",
                alias: false,
            },
        ];
        let err = validate(TABLE).expect_err("must reject the prefix collision");
        match &err {
            BindingConflict::Prefix(p) => {
                assert_eq!(p.shorter, "standalone");
                assert_eq!(p.longer, "chord");
            }
            _ => unreachable!("expected a prefix collision, got {err:?}"),
        }
    }

    #[test]
    fn validate_rejects_equal_length_duplicate_sequences() {
        const TABLE: &[Binding<TestCmd>] = &[
            Binding {
                keys: &[ctrl_k()],
                cmd: TestCmd::Standalone,
                help: "first",
                when: "",
                alias: false,
            },
            Binding {
                keys: &[ctrl_k()],
                cmd: TestCmd::Standalone,
                help: "second",
                when: "",
                alias: false,
            },
        ];
        let err = validate(TABLE).expect_err("must reject the duplicate sequence");
        match &err {
            BindingConflict::Duplicate { first, second } => {
                assert_eq!(*first, "first");
                assert_eq!(*second, "second");
            }
            _ => unreachable!("expected a duplicate-sequence conflict, got {err:?}"),
        }
    }

    #[test]
    fn validate_rejects_a_malformed_when_clause() {
        const TABLE: &[Binding<TestCmd>] = &[Binding {
            keys: &[ctrl_k()],
            cmd: TestCmd::Standalone,
            help: "malformed",
            when: "focus ==",
            alias: false,
        }];
        let err = validate(TABLE).expect_err("must reject the malformed `when` clause");
        match &err {
            BindingConflict::MalformedWhen { label, .. } => assert_eq!(*label, "malformed"),
            _ => unreachable!("expected a malformed-when conflict, got {err:?}"),
        }
    }

    /// Plan WP10.S4's coverage-gap fix: `validate` was only ever called by
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
    }

    #[test]
    fn validate_accepts_the_same_two_sequences_when_split_across_two_binding_sets() {
        // Same physical chords as the rejecting test above, but each lives
        // in its OWN table (a different binding set) — collision
        // validation is per-set, so this must succeed in BOTH directions:
        // each table validates fine entirely on its own.
        const STANDALONE_SET: &[Binding<TestCmd>] = &[Binding {
            keys: &[ctrl_k()],
            cmd: TestCmd::Standalone,
            help: "standalone",
            when: "",
            alias: false,
        }];
        const CHORD_SET: &[Binding<TestCmd>] = &[Binding {
            keys: &[ctrl_k(), ctrl_c()],
            cmd: TestCmd::Chord,
            help: "chord",
            when: "",
            alias: false,
        }];
        assert!(validate(STANDALONE_SET).is_ok());
        assert!(validate(CHORD_SET).is_ok());
    }

    #[test]
    fn validate_allows_two_bindings_with_no_prefix_relationship() {
        const TABLE: &[Binding<TestCmd>] = &[
            Binding {
                keys: &[KeyPattern::new(KeyCode::Char('a'), Mods::NONE)],
                cmd: TestCmd::Standalone,
                help: "a",
                when: "",
                alias: false,
            },
            Binding {
                keys: &[KeyPattern::new(KeyCode::Char('b'), Mods::NONE)],
                cmd: TestCmd::Chord,
                help: "b",
                when: "",
                alias: false,
            },
        ];
        assert!(validate(TABLE).is_ok());
    }
}
