//! Generic, data-driven binding tables — the second resolution style
//! alongside `keymap::resolve`. Split out of `keymap.rs` to bring that file
//! under the §1.6 500-line budget; `keymap` re-exports every item here so no
//! import path downstream changed.
//!
//! Plan WP6.S1 extended `Binding<C>` from a single `key: KeyPattern` to a
//! `keys: &'static [KeyPattern]` sequence plus a `when: &'static str` guard
//! clause (`crate::when`, `""` meaning unconditional) — the two fields the
//! sequence-capable resolver in `crate::keymap::index` needs. Every existing
//! table (`GLOBAL_BINDINGS`, `LEADER_BINDINGS`, `EXPLORER_BINDINGS`,
//! `TABS_BINDINGS`) still binds a single key each, so `resolve_in` (the
//! plain, context-free lookup those tables use) only ever matches a
//! one-element `keys` slice; the sequence- and context-aware engine that
//! actually walks a multi-key chord to completion lives in
//! `crate::keymap::index::resolve`/`resolve_stateful`.

use crate::keymap::{KeyCode, KeyInput, Mods};

/// One exact chord: a `KeyCode` plus the WHOLE `Mods` set that must be held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyPattern {
    pub code: KeyCode,
    pub mods: Mods,
}

impl KeyPattern {
    pub const fn new(code: KeyCode, mods: Mods) -> KeyPattern {
        KeyPattern { code, mods }
    }

    pub(crate) fn matches(self, key: KeyInput) -> bool {
        self.code == key.code && self.mods == key.mods
    }

    /// A short display label (footer default-mode hints, the generated Help
    /// doc — one source of truth): `^`/`⌥`/`⇧`/`⌘` for ctrl/alt/shift/sup,
    /// then the key, `Char` uppercased ("^X" for Ctrl+x).
    pub fn label(&self) -> String {
        let mut s = String::new();
        if self.mods.ctrl {
            s.push('^');
        }
        if self.mods.alt {
            s.push('\u{2325}'); // ⌥
        }
        if self.mods.shift {
            s.push('\u{21e7}'); // ⇧
        }
        if self.mods.sup {
            s.push('\u{2318}'); // ⌘
        }
        match self.code {
            KeyCode::Char(c) => s.push(c.to_ascii_uppercase()),
            KeyCode::F1 => s.push_str("F1"),
            other => s.push_str(&format!("{other:?}")),
        }
        s
    }
}

/// One table entry: the (possibly multi-key) chord sequence, the command it
/// produces, its help-line label, and a `when` clause gating it (plan
/// WP6.S1/S2). `when: ""` is the unconditional case every pre-WP6 table
/// uses — `crate::keymap::index::resolve` treats an empty clause as always
/// true without invoking the `when` parser at all.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Binding<C: Copy + 'static> {
    pub keys: &'static [KeyPattern],
    pub cmd: C,
    pub help: &'static str,
    pub when: &'static str,
}

impl<C: Copy + 'static> Binding<C> {
    /// The sequence's display label: each key's own label, space-joined
    /// ("^K ^C" for a two-key chord). A single-key binding's label is
    /// unchanged from `KeyPattern::label`.
    pub fn label(&self) -> String {
        self.keys
            .iter()
            .map(KeyPattern::label)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Linear first-match lookup over a SINGLE-key binding table — these are
/// chord tables (single- to low-double-digit entries), not per-keystroke
/// text, so a `HashMap` would cost this module's whole appeal (a `const`
/// table, no allocation) for nothing. A binding whose `keys` is a sequence
/// longer than one never matches here — callers that need sequences use
/// `crate::keymap::index::resolve`/`resolve_stateful` instead.
pub fn resolve_in<C: Copy>(table: &[Binding<C>], key: KeyInput) -> Option<C> {
    table
        .iter()
        .find(|binding| {
            binding.keys.len() == 1 && binding.keys.first().is_some_and(|k| k.matches(key))
        })
        .map(|binding| binding.cmd)
}

/// A pane's key handler's verdict on one keystroke (decision 8's four-stage
/// pipeline, `app::handle_key`): `Consumed` stops the pipeline there;
/// `Ignored` lets a later stage see the same key. `#[must_use]` — dropping
/// the verdict is indistinguishable from a bug that always consumes.
#[must_use]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyOutcome {
    Consumed,
    Ignored,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const CTRL: Mods = Mods {
        shift: false,
        alt: false,
        ctrl: true,
        sup: false,
    };

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum TestCmd {
        Foo,
    }

    #[test]
    fn resolve_in_ignores_a_multi_key_binding() {
        const TABLE: &[Binding<TestCmd>] = &[Binding {
            keys: &[
                KeyPattern::new(KeyCode::Char('k'), CTRL),
                KeyPattern::new(KeyCode::Char('c'), CTRL),
            ],
            cmd: TestCmd::Foo,
            help: "foo",
            when: "",
        }];
        let key = KeyInput {
            code: KeyCode::Char('k'),
            mods: CTRL,
        };
        assert_eq!(resolve_in(TABLE, key), None);
    }

    #[test]
    fn resolve_in_matches_a_single_key_binding() {
        const TABLE: &[Binding<TestCmd>] = &[Binding {
            keys: &[KeyPattern::new(KeyCode::Char('k'), CTRL)],
            cmd: TestCmd::Foo,
            help: "foo",
            when: "",
        }];
        let key = KeyInput {
            code: KeyCode::Char('k'),
            mods: CTRL,
        };
        assert_eq!(resolve_in(TABLE, key), Some(TestCmd::Foo));
    }

    #[test]
    fn binding_label_joins_a_sequence_with_spaces() {
        const TABLE: &[Binding<TestCmd>] = &[Binding {
            keys: &[
                KeyPattern::new(KeyCode::Char('k'), CTRL),
                KeyPattern::new(KeyCode::Char('c'), CTRL),
            ],
            cmd: TestCmd::Foo,
            help: "foo",
            when: "",
        }];
        assert_eq!(TABLE.first().expect("one entry").label(), "^K ^C");
    }
}
