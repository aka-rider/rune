//! Icon-tier selection (plan WP5): decides which `rune_md::icons::IconSet`
//! a session paints line decorations with — a nerd-font tier that needs a
//! Nerd Font or system font fallback for its private-use-area glyphs, or a
//! plain-Unicode tier that renders in any terminal font. `rune-md` only
//! ever holds the two `IconSet` VALUES (`icons.rs`'s module doc: "choosing
//! WHICH set applies is the caller's job") — this is that caller's
//! decision, made once at startup from the same kind of environment
//! signal `theme::probe`'s `COLORTERM` check reads, never re-decided per
//! frame.

use rune_md::icons::IconSet;

/// Picks the icon tier from three environment-shaped inputs, taken as
/// PARAMETERS rather than read from `std::env` directly — same reasoning
/// as `theme::probe::colorterm_claims_truecolor`'s doc comment: `RUNE_ICONS`/
/// `TERM_PROGRAM`/`TERM` are process-global, so a test that set/unset them
/// directly would race every other test in this binary running
/// concurrently. The one real caller reads the actual environment once,
/// at startup, and passes the values in here.
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
pub fn choose(env_icons: Option<&str>, term_program: Option<&str>, term: Option<&str>) -> IconSet {
    match env_icons {
        Some("nerd") => return IconSet::nerd(),
        Some("unicode") => return IconSet::unicode(),
        _ => {}
    }

    let nerd_term_program = matches!(
        term_program,
        Some("ghostty") | Some("WezTerm") | Some("iTerm.app")
    );
    let nerd_term = term.is_some_and(|t| t.contains("kitty"));

    if nerd_term_program || nerd_term {
        IconSet::nerd()
    } else {
        IconSet::unicode()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_unicode_override_beats_a_nerd_listed_terminal() {
        assert_eq!(
            choose(Some("unicode"), Some("ghostty"), None),
            IconSet::unicode()
        );
    }

    #[test]
    fn explicit_nerd_override_beats_an_unlisted_terminal() {
        assert_eq!(
            choose(Some("nerd"), Some("Apple_Terminal"), None),
            IconSet::nerd()
        );
    }

    #[test]
    fn ghostty_term_program_selects_nerd() {
        assert_eq!(choose(None, Some("ghostty"), None), IconSet::nerd());
    }

    #[test]
    fn wezterm_term_program_selects_nerd() {
        assert_eq!(choose(None, Some("WezTerm"), None), IconSet::nerd());
    }

    #[test]
    fn iterm_term_program_selects_nerd() {
        assert_eq!(choose(None, Some("iTerm.app"), None), IconSet::nerd());
    }

    #[test]
    fn kitty_term_selects_nerd() {
        assert_eq!(choose(None, None, Some("xterm-kitty")), IconSet::nerd());
    }

    #[test]
    fn everything_unset_defaults_to_unicode() {
        assert_eq!(choose(None, None, None), IconSet::unicode());
    }

    #[test]
    fn unrecognized_term_program_defaults_to_unicode() {
        assert_eq!(
            choose(None, Some("Apple_Terminal"), None),
            IconSet::unicode()
        );
    }

    #[test]
    fn unrecognized_env_icons_value_falls_through_to_term_detection() {
        assert_eq!(
            choose(Some("bogus"), Some("ghostty"), None),
            IconSet::nerd()
        );
    }
}
