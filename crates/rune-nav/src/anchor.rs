//! Anchor-name matching (CONSTITUTION §1.6 split of the crate root): the
//! comparison a `Named` anchor uses against a definition's own name.

/// Compare an in-document anchor reference against a definition's name
/// after ASCII-lowercasing and collapsing every run of ASCII whitespace to
/// a single space, trimmed both ends.
pub fn anchor_matches(anchor_name: &str, def_name: &str) -> bool {
    normalize_anchor(anchor_name) == normalize_anchor(def_name)
}

fn normalize_anchor(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending_space = false;
    for c in s.chars() {
        if c.is_ascii_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space && !out.is_empty() {
            out.push(' ');
        }
        pending_space = false;
        out.push(c.to_ascii_lowercase());
    }
    out
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
}
