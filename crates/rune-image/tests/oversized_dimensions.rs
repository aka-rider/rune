#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rune_image::decode_still;

fn encode_all_zero_grayscale_png(width: u32, height: u32) -> Vec<u8> {
    let mut png_bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut png_bytes, width, height);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::High);
    let mut writer = encoder.write_header().expect("write png header");
    let raw = vec![0u8; width as usize * height as usize];
    writer.write_image_data(&raw).expect("write image data");
    drop(writer);
    png_bytes
}

#[test]
fn decode_still_refuses_a_tiny_file_that_declares_far_more_pixels_than_the_decode_ceiling() {
    let width = 9000;
    let height = 9000;
    let data = encode_all_zero_grayscale_png(width, height);
    assert!(
        data.len() < 200_000,
        "fixture should compress to a few hundred KB on disk, got {} bytes",
        data.len()
    );

    let err = decode_still(&data).expect_err("oversized declared dimensions must be refused");
    assert!(err.to_string().to_ascii_lowercase().contains("size"));
}

#[test]
fn decode_still_still_decodes_a_file_under_the_decode_ceiling() {
    let data = encode_all_zero_grayscale_png(64, 64);
    let decoded = decode_still(&data).expect("small file must still decode");
    assert_eq!((decoded.width, decoded.height), (64, 64));
}
