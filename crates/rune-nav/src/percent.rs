//! Hand-rolled percent-decoding. Neither the `percent-encoding` nor the
//! `url` crate is a workspace dependency and both must stay out — this is
//! the entire surface a markdown link target ever needs.

/// Percent-decode `s`. Scans bytes left to right: on `%` it requires two
/// following ASCII-hex digits (either case) and pushes the decoded byte;
/// every other byte is pushed verbatim. Returns `None` on a malformed
/// escape (a `%` not followed by two hex digits) or when the decoded bytes
/// are not valid UTF-8.
pub fn decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%' {
            let hi = hex_val(*bytes.get(i + 1)?)?;
            let lo = hex_val(*bytes.get(i + 2)?)?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::decode;

    #[test]
    fn decodes_a_space_escape() {
        assert_eq!(
            decode("Canary%20tokens.md"),
            Some("Canary tokens.md".to_string())
        );
    }

    #[test]
    fn passes_through_bytes_with_no_escapes() {
        assert_eq!(decode("plain.md"), Some("plain.md".to_string()));
    }

    #[test]
    fn accepts_uppercase_and_lowercase_hex() {
        assert_eq!(decode("%2e%2E"), Some("..".to_string()));
    }

    #[test]
    fn rejects_a_percent_not_followed_by_two_hex_digits() {
        assert_eq!(decode("100%.md"), None);
    }

    #[test]
    fn rejects_a_truncated_escape_at_end_of_string() {
        assert_eq!(decode("abc%2"), None);
    }
}
