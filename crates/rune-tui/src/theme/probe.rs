//! No escape sequence reports a terminal's colour depth, so `COLORTERM` is
//! the whole signal: every truecolor-capable terminal on macOS sets it,
//! and Terminal.app — not truecolor — never does.

pub fn supports_truecolor() -> bool {
    colorterm_claims_truecolor(std::env::var("COLORTERM").ok().as_deref())
}

/// `COLORTERM` is process-global, so this takes the value as a parameter: a
/// test that set or unset it directly would race every other test running
/// concurrently in this binary.
pub(crate) fn colorterm_claims_truecolor(value: Option<&str>) -> bool {
    matches!(value, Some("truecolor") | Some("24bit"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorterm_truecolor_and_24bit_are_recognized() {
        assert!(colorterm_claims_truecolor(Some("truecolor")));
        assert!(colorterm_claims_truecolor(Some("24bit")));
    }

    #[test]
    fn anything_else_is_not_truecolor() {
        assert!(!colorterm_claims_truecolor(None));
        assert!(!colorterm_claims_truecolor(Some("")));
        assert!(!colorterm_claims_truecolor(Some("256color")));
    }
}
