//! Byte-parity: `rune-image`'s decode -> fit_box -> resize -> encode_transmit
//! pipeline reproduces the committed golden expectations' Kitty APC framing
//! exactly, verified against `tests/golden/encode_*.json` (frozen expected
//! values, updated deliberately by editing the JSON).
//!
//! PNG payload bytes are NOT portable across encoders, and two consequences
//! follow.
//!
//! First, the decoded pixel payload is compared **structurally** (base64 ->
//! PNG-decode -> compare RGBA within [`RESIZE_TOLERANCE`]) rather than by
//! bytes, to absorb CatmullRom rounding differences.
//!
//! Second — and less obvious — the CHUNK COUNT is itself non-portable,
//! because chunking splits the base64 of those same non-portable bytes: the
//! multi-chunk fixture yields 5 APCs from Go and 6 from Rust for identical
//! pixels. So an index-wise option comparison is sound ONLY for fixtures
//! small enough that both encoders stay within a single chunk, which is
//! every fixture [`check_encode_golden`] is applied to. Multi-chunk framing
//! is instead asserted as RULES, independently on each side.
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

use rune_image::{
    decode_still, encode_delete, encode_delete_all, encode_transmit, fit_box, resize,
};

/// Per-channel RGBA tolerance for comparing Rust's CatmullRom resize
/// against Go's `golang.org/x/image/draw.CatmullRom` — both are the same
/// named kernel, but the two implementations' rounding can differ by an
/// epsilon. PNG bytes themselves are never compared (see module docs).
const RESIZE_TOLERANCE: i16 = 2;

/// One parsed Kitty APC: the option string and the payload's raw text
/// (already base64, per the wire format — never re-encoded).
struct Apc {
    options: String,
    payload: String,
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn golden(name: &str) -> Value {
    let path = manifest_dir().join("tests/golden").join(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "read {}: {e} (goldens are committed expectations under tests/golden/; \
             update them deliberately by editing the JSON)",
            path.display()
        )
    });
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn asset_path(name: &str) -> PathBuf {
    manifest_dir()
        .parent()
        .expect("crates dir")
        .parent()
        .expect("repo root")
        .join("testdata/assets")
        .join(name)
}

/// Splits a (possibly chunked) Kitty APC stream into its individual
/// records, mirroring the Go dump harness's own splitter.
fn split_apcs(seq: &str) -> Vec<Apc> {
    const INTRO: &str = "\x1b_G";
    const OUTRO: &str = "\x1b\\";

    let mut out = Vec::new();
    let mut rest = seq;
    while !rest.is_empty() {
        rest = rest
            .strip_prefix(INTRO)
            .unwrap_or_else(|| panic!("malformed APC stream: missing introducer in {rest:?}"));
        let end = rest
            .find(OUTRO)
            .unwrap_or_else(|| panic!("malformed APC stream: missing terminator"));
        let body = &rest[..end];
        rest = &rest[end + OUTRO.len()..];

        let (options, payload) = match body.find(';') {
            Some(semi) => (&body[..semi], &body[semi + 1..]),
            None => (body, ""),
        };
        out.push(Apc {
            options: options.to_string(),
            payload: payload.to_string(),
        });
    }
    out
}

fn assert_rgba_matches_within_tolerance(
    got: &image::RgbaImage,
    want_w: u32,
    want_h: u32,
    want_rgba: &[u8],
    fixture: &str,
) {
    assert_eq!(
        (got.width(), got.height()),
        (want_w, want_h),
        "{fixture}: resized dimensions"
    );
    let got_raw = got.as_raw();
    assert_eq!(
        got_raw.len(),
        want_rgba.len(),
        "{fixture}: pixel buffer length"
    );
    for (i, (a, b)) in got_raw.iter().zip(want_rgba.iter()).enumerate() {
        let diff = i16::from(*a) - i16::from(*b);
        assert!(
            diff.abs() <= RESIZE_TOLERANCE,
            "{fixture}: pixel byte {i} differs by {diff} (got {a}, want {b}, tolerance {RESIZE_TOLERANCE})"
        );
    }
}

fn check_encode_golden(golden_file: &str, asset_file: &str) {
    let g = golden(golden_file);
    let id = u32::try_from(g["id"].as_u64().expect("id")).expect("id fits u32");
    let cols = g["requested_cols"].as_u64().expect("cols") as usize;
    let rows = g["requested_rows"].as_u64().expect("rows") as usize;
    let want_resized_w = g["resized_width"].as_u64().expect("resized_width") as u32;
    let want_resized_h = g["resized_height"].as_u64().expect("resized_height") as u32;
    let want_rgba = BASE64
        .decode(g["resized_rgba_b64"].as_str().expect("resized_rgba_b64"))
        .expect("decode golden rgba base64");
    let want_apcs: Vec<Apc> = g["apcs"]
        .as_array()
        .expect("apcs array")
        .iter()
        .map(|v| Apc {
            options: v["options"].as_str().expect("options").to_string(),
            payload: v["payload_b64"].as_str().expect("payload_b64").to_string(),
        })
        .collect();

    // Guard the helper's own precondition: comparing options index-wise
    // only means anything while both encoders stay inside one chunk, since
    // the chunk count follows the non-portable PNG byte length. A fixture
    // that ever grows past the boundary must move to the rule-based check
    // rather than start failing on a count mismatch.
    assert_eq!(
        want_apcs.len(),
        1,
        "{golden_file}: this helper is single-chunk only — use the \
         multi-chunk framing-rule check for a fixture that spans chunks"
    );

    let data =
        fs::read(asset_path(asset_file)).unwrap_or_else(|e| panic!("read {asset_file}: {e}"));
    let decoded = decode_still(&data).unwrap_or_else(|e| panic!("decode {asset_file}: {e}"));

    // Mirrors the Go dump harness: FitBox against the DEFAULT 8x16 cell
    // size's pixel box for the requested cols x rows.
    let (fit_w, fit_h) = fit_box(decoded.width, decoded.height, cols * 8, rows * 16);
    let resized = resize(&decoded.image, fit_w, fit_h);

    let seq = encode_transmit(&resized, id, cols, rows)
        .unwrap_or_else(|e| panic!("encode_transmit {asset_file}: {e}"));
    let apcs = split_apcs(&seq);

    assert_eq!(apcs.len(), want_apcs.len(), "{asset_file}: APC count");
    for (i, (got, want)) in apcs.iter().zip(want_apcs.iter()).enumerate() {
        assert_eq!(got.options, want.options, "{asset_file}: APC {i} options");
    }

    // Structural payload comparison: concatenate every APC's base64
    // payload, decode it, PNG-decode it, and compare RGBA within
    // tolerance — never comparing PNG bytes directly (see module docs).
    let concatenated: String = apcs.iter().map(|a| a.payload.as_str()).collect();
    let png_bytes = BASE64
        .decode(&concatenated)
        .unwrap_or_else(|e| panic!("{asset_file}: decode base64 payload: {e}"));
    let got_img = image::load_from_memory_with_format(&png_bytes, image::ImageFormat::Png)
        .unwrap_or_else(|e| panic!("{asset_file}: decode PNG payload: {e}"))
        .to_rgba8();

    assert_rgba_matches_within_tolerance(
        &got_img,
        want_resized_w,
        want_resized_h,
        &want_rgba,
        asset_file,
    );
}

#[test]
fn x_png_single_chunk() {
    check_encode_golden("encode_x_png.json", "x.png");
}

#[test]
fn y_png_square() {
    check_encode_golden("encode_y_png.json", "y.png");
}

#[test]
fn photo_jpg() {
    check_encode_golden("encode_photo_jpg.json", "photo.jpg");
}

#[test]
fn anim_gif_still_path() {
    check_encode_golden("encode_anim_gif.json", "anim.gif");
}

#[test]
fn x_png_upscale_request_does_not_upscale() {
    let g = golden("encode_x_png_upscale.json");
    let source_w = g["source_width"].as_u64().expect("source_width");
    let source_h = g["source_height"].as_u64().expect("source_height");
    let resized_w = g["resized_width"].as_u64().expect("resized_width");
    let resized_h = g["resized_height"].as_u64().expect("resized_height");
    assert_eq!(
        (resized_w, resized_h),
        (source_w, source_h),
        "an 80x40-cell request against a 64x48 source must not upscale"
    );
    check_encode_golden("encode_x_png_upscale.json", "x.png");
}

/// Asserts one APC stream obeys the reference's chunk-framing rules: a lone
/// chunk carries the full option set and NO `m=` key; a split stream is
/// full-options + `m=1`, then middles carrying ONLY `q=2,m=1`, then a final
/// `q=2,m=0`.
fn assert_chunk_framing(options: &[&str], who: &str) {
    let (first, tail) = options
        .split_first()
        .unwrap_or_else(|| panic!("{who}: empty stream"));
    if tail.is_empty() {
        assert!(
            !first.contains(",m="),
            "{who}: a single chunk must carry no m= key, got {first}"
        );
        return;
    }
    assert!(
        first.starts_with("f=100,q=2,") && first.ends_with(",m=1"),
        "{who}: first chunk options {first}"
    );
    let (last, middles) = tail
        .split_last()
        .unwrap_or_else(|| panic!("{who}: no tail"));
    for m in middles {
        assert_eq!(
            *m, "q=2,m=1",
            "{who}: a middle chunk carries only q= and m="
        );
    }
    assert_eq!(*last, "q=2,m=0", "{who}: final chunk");
}

/// The only fixture whose payload crosses the 4096-char chunk boundary.
/// Every other asset encodes as a single APC, so without this one the
/// golden corpus never exercised multi-chunk framing against the reference
/// at all — that path was covered only by Rust-side unit tests reasoning
/// about Go's behaviour, never by Go's actual output.
///
/// Note what is and is NOT portable here. The chunk COUNT is a function of
/// the PNG payload's byte length, and PNG bytes differ between Go's and
/// Rust's encoders for identical pixels (Go emits 5 APCs for this fixture,
/// Rust 6). So this asserts the framing RULES independently on each side
/// and compares decoded pixels — never the APC count, and never options by
/// index. `check_encode_golden`'s index-wise comparison is sound only for
/// the single-chunk fixtures it is applied to.
#[test]
fn noise_png_multi_chunk_framing_matches_reference_rules() {
    let g = golden("encode_noise_png.json");
    let id = u32::try_from(g["id"].as_u64().expect("id")).expect("id fits u32");
    let cols = g["requested_cols"].as_u64().expect("cols") as usize;
    let rows = g["requested_rows"].as_u64().expect("rows") as usize;

    let want_options: Vec<&str> = g["apcs"]
        .as_array()
        .expect("apcs array")
        .iter()
        .map(|a| a["options"].as_str().expect("options"))
        .collect();
    assert!(
        want_options.len() > 2,
        "noise.png must span several chunks on the Go side, got {} APC(s) — \
         the fixture is no longer incompressible enough to cover multi-chunk \
         framing",
        want_options.len()
    );
    assert_chunk_framing(&want_options, "go reference");

    let data = fs::read(asset_path("noise.png")).expect("read noise.png");
    let decoded = decode_still(&data).expect("decode noise.png");
    let (fit_w, fit_h) = fit_box(decoded.width, decoded.height, cols * 8, rows * 16);
    let resized = resize(&decoded.image, fit_w, fit_h);
    let seq = encode_transmit(&resized, id, cols, rows).expect("encode_transmit noise.png");
    let apcs = split_apcs(&seq);
    assert!(
        apcs.len() > 2,
        "rune-image must also span several chunks, got {}",
        apcs.len()
    );
    let got_options: Vec<&str> = apcs.iter().map(|a| a.options.as_str()).collect();
    assert_chunk_framing(&got_options, "rune-image");

    // Reassembling every chunk's payload must yield the original image
    // back — the real guarantee chunking has to preserve.
    let concatenated: String = apcs.iter().map(|a| a.payload.as_str()).collect();
    let png_bytes = BASE64.decode(&concatenated).expect("decode base64 payload");
    let got_img = image::load_from_memory_with_format(&png_bytes, image::ImageFormat::Png)
        .expect("decode PNG payload")
        .to_rgba8();
    let want_rgba = BASE64
        .decode(g["resized_rgba_b64"].as_str().expect("resized_rgba_b64"))
        .expect("decode golden rgba base64");
    assert_rgba_matches_within_tolerance(
        &got_img,
        g["resized_width"].as_u64().expect("resized_width") as u32,
        g["resized_height"].as_u64().expect("resized_height") as u32,
        &want_rgba,
        "noise.png",
    );
}

#[test]
fn encode_delete_matches_golden_bytes() {
    let g = golden("delete.json");
    let want = g["escape"].as_str().expect("escape");
    assert_eq!(encode_delete(42), want);
}

#[test]
fn encode_delete_all_matches_golden_bytes() {
    let g = golden("delete_all.json");
    let want = g["escape"].as_str().expect("escape");
    assert_eq!(encode_delete_all(), want);
}
