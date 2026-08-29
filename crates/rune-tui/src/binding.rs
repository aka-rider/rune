use crate::keymap::{KeyCode, KeyInput, Mods};

// `Printable` matches any printable character regardless of which one, but
// only on the table side of a match: the real terminal event always
// carries a `KeyCode`, never a `KeyMatch`, so a wildcard can never itself
// arrive as input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyMatch {
    Code(KeyCode),
    Printable,
}

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

    // A wildcard row a type-to-search table needs so the first keystroke
    // can both start the search and supply its first character.
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

    pub fn write_label(&self, out: &mut String) {
        // With `REPORT_ALTERNATE_KEYS`, a shifted chord arrives as the
        // shifted character with the `shift` bit itself cleared, so an
        // uppercase ASCII `Char` is shift on its own even when the
        // modifier is not set.
        let implicit_shift =
            matches!(self.key, KeyMatch::Code(KeyCode::Char(c)) if c.is_ascii_uppercase());
        if self.mods.ctrl {
            out.push('^');
        }
        if self.mods.alt {
            out.push('\u{2325}'); // ⌥
        }
        if self.mods.shift || implicit_shift {
            out.push('\u{21e7}'); // ⇧
        }
        if self.mods.sup {
            out.push('\u{2318}'); // ⌘
        }
        match self.key {
            KeyMatch::Code(KeyCode::Char(c)) => out.push(c.to_ascii_uppercase()),
            KeyMatch::Code(KeyCode::F1) => out.push_str("F1"),
            KeyMatch::Code(KeyCode::Backspace) => out.push('\u{232b}'), // ⌫
            KeyMatch::Code(KeyCode::Delete) => out.push('\u{2326}'),    // ⌦
            KeyMatch::Code(KeyCode::Enter) => out.push('\u{23ce}'),     // ⏎
            KeyMatch::Code(KeyCode::Escape) => out.push('\u{238b}'),    // ⎋
            KeyMatch::Code(KeyCode::Tab) => out.push('\u{21e5}'),       // ⇥
            KeyMatch::Code(KeyCode::Left) => out.push('\u{2190}'),      // ←
            KeyMatch::Code(KeyCode::Right) => out.push('\u{2192}'),     // →
            KeyMatch::Code(KeyCode::Up) => out.push('\u{2191}'),        // ↑
            KeyMatch::Code(KeyCode::Down) => out.push('\u{2193}'),      // ↓
            KeyMatch::Code(KeyCode::Home) => out.push_str("Home"),
            KeyMatch::Code(KeyCode::End) => out.push_str("End"),
            KeyMatch::Code(KeyCode::PageUp) => out.push_str("PgUp"),
            KeyMatch::Code(KeyCode::PageDown) => out.push_str("PgDn"),
            KeyMatch::Printable => out.push_str("A-Z"),
        }
    }

    pub fn label(&self) -> String {
        let mut s = String::new();
        self.write_label(&mut s);
        s
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Binding<C: Copy + 'static> {
    pub key: KeyPattern,
    pub cmd: C,
    pub help: &'static str,
    // A secondary way to reach a command that already has a primary
    // chord: the footer's hint row skips it so it does not advertise two
    // chords for the same action, but the generated Help doc still lists
    // it, since it keeps working.
    pub secondary: bool,
}

impl<C: Copy + 'static> Binding<C> {
    pub fn write_label(&self, out: &mut String) {
        self.key.write_label(out);
    }

    pub fn label(&self) -> String {
        self.key.label()
    }
}

// These are chord tables (single- to low-double-digit entries), not
// per-keystroke text, so a linear scan keeps the tables `const` with no
// allocation, and a `HashMap` would buy nothing.
//
// Precedence is first-match-wins: the earliest row in table order whose
// pattern matches is the one returned, and any later row binding the
// identical key is unreachable dead weight. This is the opposite of
// VS Code's own `keybindings.json`, which resolves last-match — a
// deliberate choice here, not an oversight.
pub fn resolve_in<C: Copy>(table: &[Binding<C>], key: KeyInput) -> Option<C> {
    table
        .iter()
        .find(|binding| binding.key.matches(key))
        .map(|binding| binding.cmd)
}

// `Consumed` stops the pipeline there; `Ignored` lets a later stage see
// the same key. `#[must_use]` because dropping the verdict is
// indistinguishable from a bug that always consumes.
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
            secondary: false,
        }];
        let key = KeyInput {
            code: KeyCode::Char('k'),
            mods: CTRL,
        };
        assert_eq!(resolve_in(TABLE, key), Some(TestCmd::Foo));
    }

    #[test]
    fn write_label_distinguishes_implicit_shift_from_base_char() {
        let base = KeyPattern::new(KeyCode::Char('g'), CTRL);
        let shifted = KeyPattern::new(KeyCode::Char('G'), CTRL);
        assert_eq!(base.label(), "^G");
        assert_eq!(shifted.label(), "^\u{21e7}G");
    }

    #[test]
    fn write_label_does_not_double_shift_prefix() {
        const SUP_SHIFT: Mods = Mods {
            shift: true,
            alt: false,
            ctrl: false,
            sup: true,
        };
        let pattern = KeyPattern::new(KeyCode::Char('|'), SUP_SHIFT);
        assert_eq!(pattern.label(), "\u{21e7}\u{2318}|");
    }

    #[test]
    fn write_label_uses_arrow_glyph_for_up() {
        let pattern = KeyPattern::new(KeyCode::Up, Mods::NONE);
        assert_eq!(pattern.label(), "\u{2191}");
    }

    #[test]
    fn write_label_prefixes_ctrl_page_up_word() {
        let pattern = KeyPattern::new(KeyCode::PageUp, CTRL);
        assert_eq!(pattern.label(), "^PgUp");
    }

    #[test]
    fn write_label_uses_escape_glyph() {
        let pattern = KeyPattern::new(KeyCode::Escape, Mods::NONE);
        assert_eq!(pattern.label(), "\u{238b}");
    }

    #[test]
    fn write_label_prefixes_ctrl_home_word() {
        let pattern = KeyPattern::new(KeyCode::Home, CTRL);
        assert_eq!(pattern.label(), "^Home");
    }
}
