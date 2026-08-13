//! Icon-tier selection (plan WP5): decides which glyph family a session
//! paints line decorations and file-tree rows with — a nerd-font tier that
//! needs a Nerd Font or system font fallback for its private-use-area
//! glyphs, or a plain-Unicode tier that renders in any terminal font. The
//! tier itself is the first-class value; `IconTier::markdown` derives the
//! `rune_md::icons::IconSet` on demand, and `crate::fileicons` derives the
//! Explorer glyph on demand — `rune-md` only ever holds the two `IconSet`
//! VALUES (its own module doc: "choosing WHICH set applies is the
//! caller's job"), and this module makes that choice once at startup from
//! the environment, never re-decided per frame.

use rune_md::icons::IconSet;

/// Which glyph family a session paints decorations with: the nerd tier
/// needs a Nerd Font (or a terminal with system font fallback covering its
/// private-use-area codepoints); the Unicode tier renders in any terminal
/// font.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconTier {
    Nerd,
    Unicode,
}

impl IconTier {
    /// The markdown decoration set this tier implies.
    pub fn markdown(self) -> IconSet {
        match self {
            IconTier::Nerd => IconSet::nerd(),
            IconTier::Unicode => IconSet::unicode(),
        }
    }
}

/// Picks the icon tier from three environment-shaped inputs, taken as
/// PARAMETERS rather than read from `std::env` directly: `RUNE_ICONS`/
/// `TERM_PROGRAM`/`TERM` are process-global, so a test that set/unset them
/// directly would race every other test in this binary running
/// concurrently.
///
/// `RUNE_ICONS=nerd`/`RUNE_ICONS=unicode` is an explicit override and wins
/// outright, whatever the terminal claims to be. Absent that, the nerd
/// tier is chosen iff `TERM_PROGRAM` names a terminal known to ship (or
/// fall back to a system font covering) the nerd-font private-use-area
/// codepoints — Ghostty, WezTerm, iTerm2 — or `TERM` contains `"kitty"`
/// (kitty sets `TERM=xterm-kitty`, not a `TERM_PROGRAM` this crate can
/// rely on). Every other terminal, including an unset environment,
/// defaults to the plain-Unicode tier — the safe choice for a terminal
/// this crate knows nothing about.
pub fn choose(env_icons: Option<&str>, term_program: Option<&str>, term: Option<&str>) -> IconTier {
    match env_icons {
        Some("nerd") => return IconTier::Nerd,
        Some("unicode") => return IconTier::Unicode,
        _ => {}
    }

    let nerd_term_program = matches!(
        term_program,
        Some("ghostty") | Some("WezTerm") | Some("iTerm.app")
    );
    let nerd_term = term.is_some_and(|t| t.contains("kitty"));

    if nerd_term_program || nerd_term {
        IconTier::Nerd
    } else {
        IconTier::Unicode
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_unicode_override_beats_a_nerd_listed_terminal() {
        assert_eq!(
            choose(Some("unicode"), Some("ghostty"), None),
            IconTier::Unicode
        );
    }

    #[test]
    fn explicit_nerd_override_beats_an_unlisted_terminal() {
        assert_eq!(
            choose(Some("nerd"), Some("Apple_Terminal"), None),
            IconTier::Nerd
        );
    }

    #[test]
    fn ghostty_term_program_selects_nerd() {
        assert_eq!(choose(None, Some("ghostty"), None), IconTier::Nerd);
    }

    #[test]
    fn wezterm_term_program_selects_nerd() {
        assert_eq!(choose(None, Some("WezTerm"), None), IconTier::Nerd);
    }

    #[test]
    fn iterm_term_program_selects_nerd() {
        assert_eq!(choose(None, Some("iTerm.app"), None), IconTier::Nerd);
    }

    #[test]
    fn kitty_term_selects_nerd() {
        assert_eq!(choose(None, None, Some("xterm-kitty")), IconTier::Nerd);
    }

    #[test]
    fn everything_unset_defaults_to_unicode() {
        assert_eq!(choose(None, None, None), IconTier::Unicode);
    }

    #[test]
    fn unrecognized_term_program_defaults_to_unicode() {
        assert_eq!(
            choose(None, Some("Apple_Terminal"), None),
            IconTier::Unicode
        );
    }

    #[test]
    fn unrecognized_env_icons_value_falls_through_to_term_detection() {
        assert_eq!(choose(Some("bogus"), Some("ghostty"), None), IconTier::Nerd);
    }
}
