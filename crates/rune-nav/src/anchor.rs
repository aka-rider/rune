//! Anchor-name matching (500-line budget split of the crate root): the
//! comparison a `Named` anchor uses against a definition's own name.

/// Compare an in-document anchor reference against a definition's name
/// after ASCII-lowercasing and collapsing every run of ASCII whitespace to
/// a single space, trimmed both ends.
pub fn anchor_matches(anchor_name: &str, def_name: &str) -> bool {
    normalize_anchor(anchor_name).eq(normalize_anchor(def_name))
}

fn normalize_anchor(s: &str) -> impl Iterator<Item = char> + '_ {
    let mut pending_space = false;
    let mut started = false;
    s.chars().flat_map(move |c| {
        if c.is_ascii_whitespace() {
            pending_space = true;
            return NormalizedChars::none();
        }
        let emit_space = pending_space && started;
        pending_space = false;
        started = true;
        if emit_space {
            NormalizedChars::two(' ', c.to_ascii_lowercase())
        } else {
            NormalizedChars::one(c.to_ascii_lowercase())
        }
    })
}

enum NormalizedChars {
    Empty,
    One(char),
    Two(char, char),
}

impl NormalizedChars {
    fn none() -> Self {
        Self::Empty
    }

    fn one(c: char) -> Self {
        Self::One(c)
    }

    fn two(a: char, b: char) -> Self {
        Self::Two(a, b)
    }
}

impl Iterator for NormalizedChars {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        match std::mem::replace(self, Self::Empty) {
            Self::Empty => None,
            Self::One(a) => Some(a),
            Self::Two(a, b) => {
                *self = Self::One(b);
                Some(a)
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn anchor_matches_ignores_case() {
        assert!(anchor_matches("Setup", "setup"));
    }

    #[test]
    fn anchor_matches_collapses_internal_whitespace_runs() {
        assert!(anchor_matches("My  Heading", "my heading"));
    }

    #[test]
    fn anchor_matches_rejects_a_genuine_mismatch() {
        assert!(!anchor_matches("Setup", "Teardown"));
    }

    #[test]
    fn anchor_matches_trims_leading_and_trailing_whitespace() {
        assert!(anchor_matches("  Setup  ", "setup"));
    }
}
