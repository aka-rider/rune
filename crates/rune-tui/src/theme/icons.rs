use rune_md::icons::IconSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconTier {
    Nerd,
    Unicode,
}

impl IconTier {
    pub fn markdown(self) -> IconSet {
        match self {
            IconTier::Nerd => IconSet::nerd(),
            IconTier::Unicode => IconSet::unicode(),
        }
    }
}

// `RUNE_ICONS`/`TERM_PROGRAM`/`TERM` are taken as parameters rather than
// read from `std::env` directly: they are process-global, so a test that
// set or unset them would race every other test running concurrently in
// this binary.
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
