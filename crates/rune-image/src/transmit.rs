//! Kitty graphics protocol APC encoding: transmit-and-display, and delete.
//!
//! Byte-parity hazards (re-derived from the vendored
//! `github.com/charmbracelet/x/ansi` kitty package and confirmed against its
//! source): PNG payload bytes are NOT portable between Go and Rust — only
//! framing, the option string, and chunk structure are asserted exactly
//! here; the payload itself is compared structurally by the caller (decode
//! base64 -> decode PNG -> compare RGBA within a tolerance).
//!
//! The reference implementation's chunking loop reads with `io.ReadFull`
//! into a fixed `MAX_CHUNK_SIZE`-byte buffer: a full read writes an in-loop
//! chunk and continues; a short read (including zero bytes, on an exact
//! multiple of the chunk size) breaks the loop WITHOUT writing, and the
//! bytes it did read are instead written as one further "last chunk" after
//! the loop. Net effect for a payload of `n` chunks of exactly
//! `MAX_CHUNK_SIZE` bytes: `n` full chunks (first carries the full option
//! set, the rest `q=2`, all with `m=1`) plus a trailing EMPTY `q=2,m=0`
//! chunk. For a payload under one chunk (including empty), the loop never
//! completes a full read: a single chunk carries the whole payload under
//! the full option set with no `m=` key at all.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

use crate::decode::ImageError;

/// The maximum chunk size, in base64 characters, applied AFTER base64 of
/// the whole payload — matching the Kitty protocol's own chunking unit.
const MAX_CHUNK_SIZE: usize = 4096;

const APC_INTRO: &str = "\x1b_G";
const APC_OUTRO: &str = "\x1b\\";

/// PNG-encodes `img`, base64s it, and frames it as one or more Kitty
/// transmit-and-put APC escapes addressed to Unicode virtual placement
/// (`U=1`) at `cols` x `rows` terminal cells. See the module docs for the
/// exact chunking/option rules.
pub fn encode_transmit(
    img: &image::RgbaImage,
    id: u32,
    cols: usize,
    rows: usize,
) -> Result<String, ImageError> {
    let mut png_bytes = Vec::new();
    {
        use image::ImageEncoder;
        let encoder = image::codecs::png::PngEncoder::new(&mut png_bytes);
        encoder.write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgba8,
        )?;
    }

    let payload = BASE64.encode(&png_bytes);
    Ok(frame_transmit(&payload, id, cols, rows))
}

/// The chunking/framing half of [`encode_transmit`], taking an
/// already-base64-encoded payload directly. Split out so the exact-
/// multiple-of-[`MAX_CHUNK_SIZE`] boundary can be exercised with a
/// synthetic payload rather than having to locate a real image whose PNG
/// encoding happens to land on it.
fn frame_transmit(payload: &str, id: u32, cols: usize, rows: usize) -> String {
    let full_options = format!("f=100,q=2,i={id},U=1,c={cols},r={rows},a=T");
    // `split_at` (not `[]`) throughout: it is a safe, non-panicking method
    // even under the workspace's `indexing_slicing` lint, and the loop
    // condition guarantees `MAX_CHUNK_SIZE <= remaining.len()` on entry.
    let mut remaining = payload.as_bytes();
    let mut out = String::new();
    let mut wrote_full_chunk = false;
    let mut is_first = true;

    while remaining.len() >= MAX_CHUNK_SIZE {
        let (chunk, rest) = remaining.split_at(MAX_CHUNK_SIZE);
        let chunk_str = std::str::from_utf8(chunk).unwrap_or_default();
        let options = if is_first {
            format!("{full_options},m=1")
        } else {
            "q=2,m=1".to_string()
        };
        out.push_str(&frame(&options, chunk_str));
        is_first = false;
        wrote_full_chunk = true;
        remaining = rest;
    }

    let remainder_str = std::str::from_utf8(remaining).unwrap_or_default();
    if wrote_full_chunk {
        // The trailing "last chunk" after the loop: whatever remained
        // (possibly empty, on an exact multiple) goes out under `q=2,m=0`.
        out.push_str(&frame("q=2,m=0", remainder_str));
    } else {
        // The loop never completed a full read: the whole (short or
        // empty) payload goes out as one chunk under the full option set,
        // with no `m=` key at all.
        out.push_str(&frame(&full_options, remainder_str));
    }
    out
}

/// Frames one APC: `ESC _ G` + options + (`;` + payload, only if
/// non-empty) + `ESC \`.
fn frame(options: &str, payload: &str) -> String {
    let mut s = String::with_capacity(APC_INTRO.len() + options.len() + payload.len() + 3);
    s.push_str(APC_INTRO);
    s.push_str(options);
    if !payload.is_empty() {
        s.push(';');
        s.push_str(payload);
    }
    s.push_str(APC_OUTRO);
    s
}

/// Returns an APC sequence that deletes the image with the given ID and
/// frees its data from the terminal.
pub fn encode_delete(id: u32) -> String {
    format!("{APC_INTRO}q=2,i={id},d=I,a=d{APC_OUTRO}")
}

/// Returns an APC sequence that deletes all images and frees their data
/// from the terminal.
pub fn encode_delete_all() -> String {
    format!("{APC_INTRO}q=2,d=A,a=d{APC_OUTRO}")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn encode_delete_matches_reference_bytes() {
        assert_eq!(encode_delete(42), "\x1b_Gq=2,i=42,d=I,a=d\x1b\\");
    }

    #[test]
    fn encode_delete_all_matches_reference_bytes() {
        assert_eq!(encode_delete_all(), "\x1b_Gq=2,d=A,a=d\x1b\\");
    }

    #[test]
    fn single_chunk_transmit_carries_no_m_key() {
        let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]));
        let seq = encode_transmit(&img, 7, 1, 1).expect("encode");
        assert!(seq.starts_with("\x1b_Gf=100,q=2,i=7,U=1,c=1,r=1,a=T;"));
        assert!(seq.ends_with("\x1b\\"));
        assert!(!seq.contains(",m="));
    }

    #[test]
    fn exact_multiple_of_chunk_size_emits_trailing_empty_apc() {
        let payload = "A".repeat(MAX_CHUNK_SIZE);
        let seq = frame_transmit(&payload, 1, 1, 1);
        assert!(seq.ends_with("\x1b_Gq=2,m=0\x1b\\"));
        // The first chunk carries the full option set plus m=1 and the
        // whole synthetic payload.
        assert!(seq.starts_with("\x1b_Gf=100,q=2,i=1,U=1,c=1,r=1,a=T,m=1;"));
    }

    #[test]
    fn two_exact_chunks_emit_two_data_apcs_and_a_trailing_empty_one() {
        let payload = "A".repeat(MAX_CHUNK_SIZE * 2);
        let seq = frame_transmit(&payload, 1, 1, 1);
        let apc_count = seq.matches(APC_INTRO).count();
        assert_eq!(apc_count, 3);
        assert!(seq.ends_with("\x1b_Gq=2,m=0\x1b\\"));
    }

    #[test]
    fn a_payload_just_over_one_chunk_carries_the_remainder_on_the_last_apc() {
        let payload = "A".repeat(MAX_CHUNK_SIZE + 10);
        let seq = frame_transmit(&payload, 1, 1, 1);
        let apc_count = seq.matches(APC_INTRO).count();
        assert_eq!(apc_count, 2);
        assert!(seq.ends_with(&format!("\x1b_Gq=2,m=0;{}\x1b\\", "A".repeat(10))));
    }

    #[test]
    fn empty_payload_is_a_single_chunk_with_no_payload_and_no_m_key() {
        let seq = frame_transmit("", 1, 1, 1);
        assert_eq!(seq, "\x1b_Gf=100,q=2,i=1,U=1,c=1,r=1,a=T\x1b\\");
    }
}
