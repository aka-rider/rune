#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;

fn raw(b: u8) -> char {
    raw_byte_char(b).expect("byte is >= 0x80")
}

fn raws(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| raw(b)).collect()
}

#[test]
fn empty_input_decodes_to_empty_text() {
    assert_eq!(decode(Vec::new()), "");
    assert!(encode("").is_empty());
}

#[test]
fn all_0xff_becomes_one_raw_char_per_byte() {
    let text = decode(vec![0xFF; 64]);
    assert_eq!(text.chars().count(), 64);
    assert!(text.chars().all(|c| c == '\u{10FFFF}'));
    assert_eq!(encode(&text).as_ref(), [0xFF; 64]);
}

#[test]
fn lone_continuation_bytes_are_raw() {
    assert_eq!(decode(vec![0x80, 0xBF]), raws(&[0x80, 0xBF]));
}

#[test]
fn truncated_sequence_at_eof_is_raw_per_byte() {
    assert_eq!(
        decode(vec![b'a', 0xE2, 0x82]),
        format!("a{}", raws(&[0xE2, 0x82]))
    );
}

#[test]
fn overlong_encodings_are_raw() {
    assert_eq!(decode(vec![0xC0, 0x80]), raws(&[0xC0, 0x80]));
    assert_eq!(decode(vec![0xE0, 0x80, 0x80]), raws(&[0xE0, 0x80, 0x80]));
}

#[test]
fn encoded_surrogate_is_raw() {
    assert_eq!(decode(vec![0xED, 0xA0, 0x80]), raws(&[0xED, 0xA0, 0x80]));
}

#[test]
fn byte_order_mark_followed_by_invalid_byte_keeps_both() {
    let text = decode(vec![0xEF, 0xBB, 0xBF, 0xFF]);
    assert_eq!(text, format!("\u{feff}{}", raw(0xFF)));
    assert_eq!(encode(&text).as_ref(), [0xEF, 0xBB, 0xBF, 0xFF]);
}

#[test]
fn native_raw_range_char_is_escaped_into_four_raw_chars() {
    let text = decode(vec![0xF4, 0x8F, 0xBE, 0x80]);
    assert_eq!(text, raws(&[0xF4, 0x8F, 0xBE, 0x80]));
    assert_eq!(text.chars().count(), 4);
    assert_eq!(encode(&text).as_ref(), [0xF4, 0x8F, 0xBE, 0x80]);
}

#[test]
fn valid_plane_16_char_below_the_raw_range_is_kept() {
    let bytes = "\u{10FF7F}".as_bytes().to_vec();
    assert_eq!(decode(bytes), "\u{10FF7F}");
}

#[test]
fn latin_1_cafe_keeps_ascii_and_marks_the_accent_byte() {
    let text = decode(b"caf\xE9".to_vec());
    assert_eq!(text, format!("caf{}", raw(0xE9)));
    assert_eq!(encode(&text).as_ref(), b"caf\xE9");
}

#[test]
fn valid_input_without_the_raw_lead_is_decoded_without_copying() {
    let bytes = "h\u{e9}llo \u{263a}".as_bytes().to_vec();
    let ptr = bytes.as_ptr();
    let text = decode(bytes);
    assert_eq!(text.as_ptr(), ptr);
}

#[test]
fn encode_borrows_text_without_raw_chars() {
    assert!(matches!(encode("plain \u{263a}"), Cow::Borrowed(_)));
    assert!(matches!(encode("\u{10FF7F}"), Cow::Owned(_)));
    assert!(matches!(encode("\u{10FFFF}"), Cow::Owned(_)));
}

#[test]
fn raw_byte_mapping_covers_exactly_the_top_128_scalars() {
    assert_eq!(raw_byte_char(0x7F), None);
    assert_eq!(raw_byte_char(0x80), Some(RAW_BYTE_FIRST));
    assert_eq!(raw_byte_char(0xFF), Some('\u{10FFFF}'));
    assert_eq!(raw_byte_value('\u{10FF7F}'), None);
    assert_eq!(raw_byte_value('a'), None);
    assert_eq!(raw_byte_value(RAW_BYTE_FIRST), Some(0x80));
    assert_eq!(raw_byte_value('\u{10FFFF}'), Some(0xFF));
    assert!(!is_raw_byte('\u{10FF7F}'));
    assert!(is_raw_byte(RAW_BYTE_FIRST));
    for b in 0x80..=0xFFu8 {
        assert_eq!(raw_byte_char(b).and_then(raw_byte_value), Some(b));
    }
}

#[test]
fn count_raw_bytes_counts_only_raw_chars() {
    assert_eq!(count_raw_bytes(""), 0);
    assert_eq!(count_raw_bytes("caf\u{e9}"), 0);
    assert_eq!(count_raw_bytes(&decode(b"a\xFFb\xFEc".to_vec())), 2);
}

#[test]
fn glyph_is_lowercase_hex_in_angle_brackets() {
    assert_eq!(glyph(0xFF), "<ff>");
    assert_eq!(glyph(0x80), "<80>");
    assert_eq!(glyph(0xE9), "<e9>");
}
