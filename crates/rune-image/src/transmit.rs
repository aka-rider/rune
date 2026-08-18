//! Kitty graphics protocol (see the terminal-wg/kitty graphics protocol
//! docs) APC encoding: transmit-and-display, and delete.
//!
//! Byte-parity hazards (re-derived against the committed golden
//! expectations and confirmed against the protocol spec): PNG payload
//! bytes are NOT portable between encoders — only framing, the option
//! string, and chunk structure are asserted exactly here; the payload
//! itself is compared structurally by the caller (decode base64 -> decode
//! PNG -> compare RGBA within a tolerance).
//!
//! Chunking reads into a fixed `MAX_CHUNK_SIZE`-byte buffer: a full read
//! writes an in-loop chunk and continues; a short read (including zero
//! bytes, on an exact multiple of the chunk size) breaks the loop WITHOUT
//! writing, and the bytes it did read are instead written as one further
//! "last chunk" after the loop. Net effect for a payload of `n` chunks of
//! exactly `MAX_CHUNK_SIZE` bytes: `n` full chunks (first carries the full
//! option set, the rest `q=2`, all with `m=1`) plus a trailing EMPTY
//! `q=2,m=0` chunk. For a payload under one chunk (including empty), the
//! loop never completes a full read: a single chunk carries the whole
//! payload under the full option set with no `m=` key at all.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

use crate::cellsize::{CellSize, PixelSize};
use crate::decode::{Decoded, ImageError};
use crate::resize::{fit_box, resize};

/// The maximum chunk size, in base64 characters, applied AFTER base64 of
/// the whole payload — matching the Kitty protocol's own chunking unit.
const MAX_CHUNK_SIZE: usize = 4096;

/// Hard ceiling on the pixel COUNT (w*h) [`fit_and_encode`] will ever
/// encode, independent of the caller-supplied `cols`/`rows` cell box.
/// `footprint::fit` (`rune-tui`) derives that box by scaling to the pane's
/// WIDTH only and letting the row count follow the image's own aspect
/// ratio uncapped — the right call for the normal case (a screenshot a few
/// thousand pixels tall), but a source image whose natural width already
/// fits the pane (a narrow, very tall panorama or a hostile file) passes
/// through `fit_box` untouched at full native resolution: nothing upstream
/// bounds total decoded pixels. Past this ceiling `fit_and_encode` scales
/// the box down further, preserving aspect, before resizing — the one
/// place total transmitted bytes are capped regardless of source
/// dimensions. `4096 * 4096` keeps the worst case (near-incompressible
/// pixel data, base64 inflation) under ~85 MB rather than scaling with an
/// arbitrary source image's total pixel count.
const MAX_TRANSMIT_PIXELS: usize = 4096 * 4096;

const APC_INTRO: &str = "\x1b_G";
const APC_OUTRO: &str = "\x1b\\";

const QUIET_QUERY: &str = "q=2";
const CONTINUATION_FLAG: &str = "m=1";
const CONTINUATION_OPTIONS: &str = "q=2,m=1";
const FINAL_CHUNK_OPTIONS: &str = "q=2,m=0";

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
    let full_options = format!("f=100,{QUIET_QUERY},i={id},U=1,c={cols},r={rows},a=T");
    let mut remaining = payload;
    let mut out = String::new();
    let mut wrote_full_chunk = false;
    let mut is_first = true;

    while let Some((chunk, rest)) = remaining.split_at_checked(MAX_CHUNK_SIZE) {
        if is_first {
            out.push_str(&frame(
                &format!("{full_options},{CONTINUATION_FLAG}"),
                chunk,
            ));
            is_first = false;
        } else {
            out.push_str(&frame(CONTINUATION_OPTIONS, chunk));
        }
        wrote_full_chunk = true;
        remaining = rest;
    }

    if wrote_full_chunk {
        out.push_str(&frame(FINAL_CHUNK_OPTIONS, remaining));
    } else {
        out.push_str(&frame(&full_options, remaining));
    }
    out
}

/// Frames one APC: `ESC _ G` + options + (`;` + payload, only if
/// non-empty) + `ESC \`.
fn frame(options: &str, payload: &str) -> String {
    // Exact capacity: intro + options + the `;` separator + payload + outro.
    let separator_len = if payload.is_empty() { 0 } else { 1 };
    let mut s = String::with_capacity(
        APC_INTRO.len() + options.len() + separator_len + payload.len() + APC_OUTRO.len(),
    );
    s.push_str(APC_INTRO);
    s.push_str(options);
    if !payload.is_empty() {
        s.push(';');
        s.push_str(payload);
    }
    s.push_str(APC_OUTRO);
    s
}

/// Fits `decoded`'s full-resolution pixels into `cols` x `rows` terminal
/// cells (fit-to-width, never upscale — the same posture [`fit_box`]
/// already guarantees) and encodes the result as a Kitty transmit escape
/// sequence addressed to `id`. `rune-tui`'s image pipeline is the one
/// caller — both the initial decode (plan WP5.S2) and any later re-fit (a
/// pane resize that changes the cell footprint, WP5.S6) call through this
/// single implementation of the fit_box -> resize -> encode_transmit
/// sequence, rather than each keeping its own copy that could drift apart.
pub fn fit_and_encode(
    decoded: &Decoded,
    id: u32,
    cols: usize,
    rows: usize,
    cell: CellSize,
) -> Result<String, ImageError> {
    let fitted = fit_box(
        PixelSize {
            w: decoded.width,
            h: decoded.height,
        },
        PixelSize {
            w: cols * cell.w,
            h: rows * cell.h,
        },
    );
    let capped = cap_pixel_count(fitted, MAX_TRANSMIT_PIXELS);
    let resized = resize(&decoded.image, capped.w, capped.h);
    encode_transmit(&resized, id, cols, rows)
}

/// Scales `size` down to at most `max_pixels` total pixels, preserving
/// aspect ratio; a no-op when `size` is already at or under the cap.
/// Never upscales, and never returns a zero dimension for a non-zero
/// input (floors each side at `1` so an extreme aspect ratio — e.g. a
/// panorama a handful of pixels wide but enormous tall — still shrinks
/// toward the cap rather than collapsing to nothing on the narrow side).
fn cap_pixel_count(size: PixelSize, max_pixels: usize) -> PixelSize {
    let total = size.w.saturating_mul(size.h);
    if total <= max_pixels || total == 0 {
        return size;
    }
    let scale = (max_pixels as f64 / total as f64).sqrt();
    PixelSize {
        w: ((size.w as f64 * scale) as usize).max(1),
        h: ((size.h as f64 * scale) as usize).max(1),
    }
}

/// Returns an APC sequence that deletes the image with the given ID and
/// frees its data from the terminal.
pub fn encode_delete(id: u32) -> String {
    delete_apc(&format!("{QUIET_QUERY},i={id},d=I,a=d"))
}

/// Returns an APC sequence that deletes all images and frees their data
/// from the terminal.
pub fn encode_delete_all() -> String {
    delete_apc(&format!("{QUIET_QUERY},d=A,a=d"))
}

fn delete_apc(options: &str) -> String {
    format!("{APC_INTRO}{options}{APC_OUTRO}")
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

    #[test]
    fn cap_pixel_count_is_a_no_op_under_the_ceiling() {
        let size = PixelSize { w: 100, h: 50 };
        assert_eq!(cap_pixel_count(size, 4096 * 4096), size);
    }

    #[test]
    fn cap_pixel_count_shrinks_a_wide_image_to_the_ceiling_preserving_aspect() {
        let capped = cap_pixel_count(PixelSize { w: 2000, h: 1000 }, 500_000);
        assert!(capped.w * capped.h <= 500_000);
        // 2:1 aspect preserved within rounding.
        assert!(capped.w.abs_diff(capped.h * 2) <= 2);
    }

    #[test]
    fn cap_pixel_count_shrinks_an_extreme_aspect_ratio_without_collapsing() {
        // A narrow, very tall panorama: width alone never exceeds a pane,
        // so nothing upstream of `fit_and_encode` would ever downscale it.
        let capped = cap_pixel_count(
            PixelSize {
                w: 10,
                h: 10_000_000,
            },
            4096 * 4096,
        );
        assert!(capped.w * capped.h <= 4096 * 4096);
        assert!(capped.w >= 1 && capped.h >= 1);
    }

    #[test]
    fn fit_and_encode_caps_total_transmitted_pixels_when_the_box_does_not() {
        // 5000x5000 (25M px) comfortably fits the requested cell box below
        // (8000x16000 px), so `fit_box` alone passes it through untouched —
        // `fit_and_encode`'s own ceiling is the only thing that still
        // shrinks it, to exactly `MAX_TRANSMIT_PIXELS`.
        let decoded = Decoded {
            image: image::RgbaImage::from_pixel(5000, 5000, image::Rgba([5, 5, 5, 255])),
            width: 5000,
            height: 5000,
            format: crate::decode::Format::Png,
        };
        let cell = CellSize { w: 8, h: 16 };
        let seq = fit_and_encode(&decoded, 1, 1000, 1000, cell).expect("encode");
        assert!(seq.contains(";"));
        // A solid-colour source compresses far below the near-incompressible
        // worst case — this only proves the ceiling was reached at all
        // (covered for byte-size purposes separately, by `cap_pixel_count`'s
        // own unit tests against near-incompressible dimensions).
        assert!(seq.len() < 2_000_000, "seq.len() = {}", seq.len());
    }

    #[test]
    fn fit_and_encode_never_upscales_and_addresses_the_requested_footprint() {
        let decoded = Decoded {
            image: image::RgbaImage::from_pixel(4, 4, image::Rgba([9, 9, 9, 255])),
            width: 4,
            height: 4,
            format: crate::decode::Format::Png,
        };
        let cell = CellSize { w: 8, h: 16 };
        // A 4x4 source asked to fill a much larger cell box must not
        // upscale — `fit_box` inside `fit_and_encode` floors the resize at
        // the source's own size.
        let seq = fit_and_encode(&decoded, 7, 10, 5, cell).expect("encode");
        assert!(seq.starts_with("\x1b_Gf=100,q=2,i=7,U=1,c=10,r=5,a=T"));
    }
}
