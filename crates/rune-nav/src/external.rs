//! The external-scheme allowlist (500-line budget split of the crate
//! root) — the one predicate `resolve` trusts to decide whether a `Target`
//! opens through the OS opener rather than the filesystem.

/// THIS IS A SECURITY BOUNDARY: it is the allowlist gating a later `open(1)`
/// process spawn, so only these three schemes may ever pass — `file://`,
/// `javascript:`, `data:` and `ftp://` must never be treated as external.
/// Returns the exact string that was approved (trimmed, original case
/// preserved) so a caller can never accidentally dispatch some other,
/// unapproved spelling of the same target — the predicate and the value
/// that reaches `/usr/bin/open` are the same value.
pub fn is_external(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let lowered = trimmed.to_ascii_lowercase();
    if lowered.starts_with("http://")
        || lowered.starts_with("https://")
        || lowered.starts_with("mailto:")
    {
        Some(trimmed.to_string())
    } else {
        None
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn is_external_accepts_the_three_allowed_schemes_case_insensitively() {
        assert_eq!(
            is_external("http://example.com"),
            Some("http://example.com".to_string())
        );
        assert_eq!(
            is_external("https://example.com"),
            Some("https://example.com".to_string())
        );
        assert_eq!(
            is_external("mailto:someone@example.com"),
            Some("mailto:someone@example.com".to_string())
        );
        assert_eq!(
            is_external("HTTP://example.com"),
            Some("HTTP://example.com".to_string())
        );
    }

    #[test]
    fn is_external_rejects_every_other_scheme() {
        assert_eq!(is_external("file:///etc/passwd"), None);
        assert_eq!(is_external("javascript:alert(1)"), None);
        assert_eq!(is_external("data:text/plain;base64,aGk="), None);
        assert_eq!(is_external("ftp://example.com"), None);
    }
}
