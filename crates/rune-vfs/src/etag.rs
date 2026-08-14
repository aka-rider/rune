use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Etag(String);

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MalformedEtag(String);

impl std::fmt::Display for MalformedEtag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?} is not a lowercase SHA-256 hex digest", self.0)
    }
}

impl std::error::Error for MalformedEtag {}

impl Etag {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn from_stored(hash: impl Into<String>) -> Result<Etag, MalformedEtag> {
        let hash = hash.into();
        if is_sha256_hex(&hash) {
            Ok(Etag(hash))
        } else {
            Err(MalformedEtag(hash))
        }
    }
}

impl std::fmt::Display for Etag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub fn etag_of(bytes: &[u8]) -> Etag {
    use std::fmt::Write as _;

    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let sum = hasher.finalize();
    let mut out = String::with_capacity(sum.len() * 2);
    for byte in sum {
        let _ = write!(out, "{byte:02x}");
    }
    Etag(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn etag_of_empty_matches_known_sha256() {
        let etag = etag_of(b"");
        assert_eq!(
            etag.as_str(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn etag_of_differs_on_different_content() {
        assert_ne!(etag_of(b"hello"), etag_of(b"world"));
    }

    #[test]
    fn etag_display_is_lowercase_hex() {
        let etag = etag_of(b"hello");
        assert!(etag.to_string().chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(etag.to_string(), etag.as_str());
    }

    #[test]
    fn from_stored_accepts_a_real_sha256_hex_digest() {
        let etag = Etag::from_stored(etag_of(b"hello").to_string()).unwrap();
        assert_eq!(etag, etag_of(b"hello"));
    }

    #[test]
    fn from_stored_rejects_a_malformed_hash() {
        let err = Etag::from_stored("not-a-hash").unwrap_err();
        assert!(
            err.to_string()
                .contains("not a lowercase SHA-256 hex digest")
        );
    }
}
