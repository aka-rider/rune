//! Generic, data-driven binding tables — the second resolution style
//! alongside `keymap::resolve`. Split out of `keymap.rs` to bring that file
//! under the §1.6 500-line budget; `keymap` re-exports every item here so no
//! import path downstream changed.

use crate::keymap::{KeyCode, KeyInput, Mods};

// ── Generic binding tables (plan WP2.S3/decision 9) ─────────────────────
//
// A second, data-driven resolution style alongside `keymap::resolve`:
// `KeyPattern` matches a chord EXACTLY (code + the WHOLE `Mods` set, unlike
// `resolve_char`'s partial guards), and `resolve_in` looks one up in a
// `const` table. `resolve` itself stays hand-written (decision 9: "tables
// only where WP7's Help doc needs enumeration") — `GLOBAL_BINDINGS`, and
// `EXPLORER_BINDINGS`/`TABS_BINDINGS`, are that case.

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

    fn matches(self, key: KeyInput) -> bool {
        self.code == key.code && self.mods == key.mods
    }

    /// A short display label (footer default-mode hints, WP7's Help doc —
    /// one source of truth): `^`/`⌥`/`⇧`/`⌘` for ctrl/alt/shift/sup, then
    /// the key, `Char` uppercased ("^X" for Ctrl+x).
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

/// One table entry: the chord, the command it produces, and its help-line
/// label — `help` is the one source the footer's hints and WP7's Help doc
/// both read.
#[derive(Clone, Copy, Debug)]
pub struct Binding<C: Copy + 'static> {
    pub key: KeyPattern,
    pub cmd: C,
    pub help: &'static str,
}

/// Linear first-match lookup — these are chord tables (single- to low-
/// double-digit entries), not per-keystroke text, so a `HashMap` would cost
/// this module's whole appeal (a `const` table, no allocation) for nothing.
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
