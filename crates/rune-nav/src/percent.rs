//! Hand-rolled percent-decoding. Neither the `percent-encoding` nor the
//! `url` crate is a workspace dependency and both must stay out — this is
//! the entire surface a markdown link target ever needs.

/// Percent-decode `s`, infallibly: on `%` followed by two ASCII-hex digits
/// (either case), the decoded byte is pushed; any other `%` — end of
/// input, or not followed by two hex digits — is pushed verbatim and
/// scanning resumes at the next byte, so a malformed escape never discards
/// the rest of the string (this also means `decode` is always safe to use
/// as the sole candidate: whenever `s` has no valid escapes, `decode(s) ==
/// s`). If the assembled bytes are not valid UTF-8 — only reachable via a
/// well-formed escape decoding into a stray continuation byte — the
/// invalid sequences are lossily replaced rather than failing the whole
/// decode.
pub(crate) fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%' {
            if let (Some(hi), Some(lo)) = (
                bytes.get(i + 1).copied().and_then(hex_val),
                bytes.get(i + 2).copied().and_then(hex_val),
            ) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
            out.push(b);
            i += 1;
            continue;
        }
        out.push(b);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
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
        assert_eq!(decode("Canary%20tokens.md"), "Canary tokens.md".to_string());
    }

    #[test]
    fn passes_through_bytes_with_no_escapes() {
        assert_eq!(decode("plain.md"), "plain.md".to_string());
    }

    #[test]
    fn accepts_uppercase_and_lowercase_hex() {
        assert_eq!(decode("%2e%2E"), "..".to_string());
    }

    #[test]
    fn a_percent_not_followed_by_two_hex_digits_passes_through_verbatim() {
        assert_eq!(decode("100%.md"), "100%.md".to_string());
    }

    #[test]
    fn a_truncated_escape_at_end_of_input_passes_through_verbatim() {
        assert_eq!(decode("abc%2"), "abc%2".to_string());
    }

    #[test]
    fn a_bare_trailing_percent_passes_through_verbatim() {
        assert_eq!(decode("abc%"), "abc%".to_string());
    }

    #[test]
    fn a_multibyte_utf8_character_reassembles_across_consecutive_escapes() {
        // "café" — the 'é' is the two-byte UTF-8 sequence 0xC3 0xA9, each
        // byte independently percent-escaped.
        assert_eq!(decode("caf%C3%A9"), "café".to_string());
    }

    #[test]
    fn an_escape_decoding_to_a_lone_continuation_byte_is_lossily_replaced() {
        // %80 alone is a UTF-8 continuation byte with no leading byte —
        // invalid on its own; decode must not fail the whole string.
        let decoded = decode("bad%80name.md");
        assert!(decoded.contains("bad"));
        assert!(decoded.contains("name.md"));
    }

    #[test]
    fn a_null_escape_decodes_to_a_literal_nul_byte() {
        assert_eq!(decode("%00"), "\u{0}".to_string());
    }
}
