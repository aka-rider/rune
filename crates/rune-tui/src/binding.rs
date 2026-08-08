//! Generic, data-driven binding tables — the second resolution style
//! alongside `keymap::resolve`. Split out of `keymap.rs` to bring that file
//! under the 500-line budget; `keymap` re-exports every item here so no
//! import path downstream changed.

use crate::keymap::{KeyCode, KeyInput, Mods};

/// What a table row matches: one exact `KeyCode`, or any printable
/// character — the Explorer's type-to-search row (plan "Explorer
/// type-to-search", S1). Deliberately pattern-side only: `KeyInput` (the
/// real terminal event `app::handle_key` receives) always carries a real
/// `KeyCode`, never a `KeyMatch`, so a wildcard can never itself arrive AS
/// input — it can only ever sit on the table side of a match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyMatch {
    Code(KeyCode),
    Printable,
}

/// One exact chord: a `KeyMatch` plus the WHOLE `Mods` set that must be
/// held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyPattern {
    pub key: KeyMatch,
    pub mods: Mods,
}

impl KeyPattern {
    pub const fn new(code: KeyCode, mods: Mods) -> KeyPattern {
        KeyPattern {
            key: KeyMatch::Code(code),
            mods,
        }
    }

    /// A row that matches any printable `Char` (`!c.is_control()`) under
    /// the given `mods`, with no regard to which character it is — the
    /// wildcard `EXPLORER_SEARCH_BINDINGS`'s `Type` row needs so the FIRST
    /// keystroke can both start a search and supply its first character.
    pub const fn printable(mods: Mods) -> KeyPattern {
        KeyPattern {
            key: KeyMatch::Printable,
            mods,
        }
    }

    pub(crate) fn matches(self, key: KeyInput) -> bool {
        if self.mods != key.mods {
            return false;
        }
        match self.key {
            KeyMatch::Code(code) => code == key.code,
            KeyMatch::Printable => matches!(key.code, KeyCode::Char(c) if !c.is_control()),
        }
    }

    /// A short display label (footer default-mode hints, the generated Help
    /// doc — one source of truth): `^`/`⌥`/`⇧`/`⌘` for ctrl/alt/shift/sup,
    /// then the key, `Char` uppercased ("^X" for Ctrl+x). `Printable` has no
    /// single character to show, so it renders as the class it matches.
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
        match self.key {
            KeyMatch::Code(KeyCode::Char(c)) => s.push(c.to_ascii_uppercase()),
            KeyMatch::Code(KeyCode::F1) => s.push_str("F1"),
            KeyMatch::Code(KeyCode::Backspace) => s.push('\u{232b}'), // ⌫
            KeyMatch::Code(other) => s.push_str(&format!("{other:?}")),
            KeyMatch::Printable => s.push_str("A-Z"),
        }
        s
    }
}

/// One table entry: the chord it binds, the command it produces, and its
/// help-line label.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Binding<C: Copy + 'static> {
    pub key: KeyPattern,
    pub cmd: C,
    pub help: &'static str,
    /// A secondary way to reach a command that already has a primary chord.
    /// The footer's hint row skips an aliased binding so it does not
    /// advertise two chords for the same action; the generated Help doc
    /// still lists it, since it keeps working.
    pub alias: bool,
}

impl<C: Copy + 'static> Binding<C> {
    pub fn label(&self) -> String {
        self.key.label()
    }
}

/// Linear first-match lookup over a binding table — these are chord tables
/// (single- to low-double-digit entries), not per-keystroke text, so a
/// `HashMap` would cost this module's whole appeal (a `const` table, no
/// allocation) for nothing.
///
/// Precedence is FIRST-MATCH-WINS: the earliest row in table order whose
/// pattern matches is the one returned, and any later row binding the
/// identical key is unreachable dead weight (`crate::keymap::index::
/// validate` rejects that shape outright, so it can only happen inside a
/// table that skipped validation). This is the opposite of VS Code's own
/// `keybindings.json`, which resolves LAST-match — a deliberate choice
/// here, not an oversight, so state it once rather than leave it
/// undocumented for whoever next reaches for the VS Code precedent.
pub fn resolve_in<C: Copy>(table: &[Binding<C>], key: KeyInput) -> Option<C> {
    table
        .iter()
        .find(|binding| binding.key.matches(key))
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
    fn resolve_in_matches_a_single_key_binding() {
        const TABLE: &[Binding<TestCmd>] = &[Binding {
            key: KeyPattern::new(KeyCode::Char('k'), CTRL),
            cmd: TestCmd::Foo,
            help: "foo",
            alias: false,
        }];
        let key = KeyInput {
            code: KeyCode::Char('k'),
            mods: CTRL,
        };
        assert_eq!(resolve_in(TABLE, key), Some(TestCmd::Foo));
    }
}
