#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use proptest::prelude::*;
use rune_core::buffer::Buffer;
use rune_core::codec::{decode, encode};

fn biased_bytes() -> impl Strategy<Value = Vec<u8>> {
    let segment = prop_oneof![
        "\\PC{0,8}".prop_map(String::into_bytes),
        Just(vec![0xF4, 0x8F, 0xBE]),
        Just(vec![0xF4, 0x8F, 0xBF]),
        (0x80u8..=0xFF).prop_map(|b| vec![b]),
        any::<u8>().prop_map(|b| vec![b]),
    ];
    prop::collection::vec(segment, 0..24).prop_map(|parts| parts.concat())
}

proptest! {
    #[test]
    fn encode_inverts_decode_for_any_bytes(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        let text = decode(bytes.clone());
        prop_assert_eq!(encode(&text), bytes.as_slice());
    }

    #[test]
    fn encode_inverts_decode_for_biased_bytes(bytes in biased_bytes()) {
        let text = decode(bytes.clone());
        prop_assert_eq!(encode(&text), bytes.as_slice());
    }

    #[test]
    fn decode_inverts_encode_for_decoded_text(bytes in biased_bytes()) {
        let text = decode(bytes);
        prop_assert_eq!(decode(encode(&text).into_owned()), text);
    }

    #[test]
    fn buffer_disk_bytes_equal_the_loaded_bytes(bytes in biased_bytes()) {
        let buffer = Buffer::from_bytes(bytes.clone());
        prop_assert_eq!(buffer.disk_bytes(), bytes.as_slice());
    }
}

#[test]
fn insert_and_delete_on_a_latin_1_fixture_round_trip() {
    let original = b"caf\xE9 cr\xE8me\n".to_vec();
    let buffer = Buffer::from_bytes(original.clone());

    let edited = buffer.insert(0, "> ").expect("insert at start");
    assert_eq!(edited.disk_bytes().as_ref(), b"> caf\xE9 cr\xE8me\n");

    let restored = edited.delete(0, 2).expect("delete the inserted prefix");
    assert_eq!(restored.disk_bytes().as_ref(), original.as_slice());
}

#[test]
fn deleting_a_raw_byte_removes_exactly_that_byte() {
    let buffer = Buffer::from_bytes(b"a\xE9b".to_vec());
    let raw_len = '\u{10FFE9}'.len_utf8();
    let edited = buffer.delete(1, 1 + raw_len).expect("delete the raw char");
    assert_eq!(edited.disk_bytes().as_ref(), b"ab");
}
