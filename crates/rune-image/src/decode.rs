use std::io::Cursor;

pub use image::ImageError;

pub const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;

const MAX_DECODE_PIXELS: u64 = 4 * crate::transmit::MAX_TRANSMIT_PIXELS as u64;
const MAX_DECODE_AXIS: u32 = 16_384;

const SVG_SNIFF_WINDOW: usize = 512;

#[derive(Debug)]
pub struct Decoded {
    pub image: image::RgbaImage,
    pub width: usize,
    pub height: usize,
    pub format: Format,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Gif,
    Bmp,
    Tiff,
    WebP,
    Svg,
}

pub fn extensions() -> &'static [&'static str] {
    #[cfg(feature = "svg")]
    {
        &[
            "png", "jpg", "jpeg", "gif", "bmp", "tif", "tiff", "webp", "svg",
        ]
    }
    #[cfg(not(feature = "svg"))]
    {
        &["png", "jpg", "jpeg", "gif", "bmp", "tif", "tiff", "webp"]
    }
}

pub fn sniff_format(data: &[u8]) -> Option<Format> {
    if looks_like_svg(data) {
        return Some(Format::Svg);
    }
    let format = image::guess_format(data).ok()?;
    from_image_format(format)
}

fn from_image_format(format: image::ImageFormat) -> Option<Format> {
    match format {
        image::ImageFormat::Png => Some(Format::Png),
        image::ImageFormat::Jpeg => Some(Format::Jpeg),
        image::ImageFormat::Gif => Some(Format::Gif),
        image::ImageFormat::Bmp => Some(Format::Bmp),
        image::ImageFormat::Tiff => Some(Format::Tiff),
        image::ImageFormat::WebP => Some(Format::WebP),
        _ => None,
    }
}

fn looks_like_svg(data: &[u8]) -> bool {
    let without_bom = data.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(data);
    let window_len = without_bom.len().min(SVG_SNIFF_WINDOW);
    let window = without_bom.get(..window_len).unwrap_or(&[]);
    let text = match std::str::from_utf8(window) {
        Ok(text) => text,
        Err(err) => window
            .get(..err.valid_up_to())
            .and_then(|valid| std::str::from_utf8(valid).ok())
            .unwrap_or(""),
    };
    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        return false;
    }
    let low = trimmed.to_ascii_lowercase();
    if low.starts_with("<svg") {
        return true;
    }
    if low.starts_with("<?xml") || low.starts_with("<!doctype") || low.starts_with("<!--") {
        return low.contains("<svg");
    }
    false
}

fn dimension_limit_error() -> ImageError {
    ImageError::Limits(image::error::LimitError::from_kind(
        image::error::LimitErrorKind::DimensionError,
    ))
}

fn decode_limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DECODE_AXIS);
    limits.max_image_height = Some(MAX_DECODE_AXIS);
    limits
}

#[cfg(not(feature = "svg"))]
fn svg_unsupported_error() -> ImageError {
    ImageError::IoError(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "svg support not built into this binary",
    ))
}

pub fn decode_still(data: &[u8]) -> Result<Decoded, ImageError> {
    if data.is_empty() {
        return Err(ImageError::IoError(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "decode image: empty data",
        )));
    }

    #[cfg(feature = "svg")]
    if looks_like_svg(data) {
        return crate::svg::decode_svg(data).map_err(|err| {
            ImageError::IoError(std::io::Error::new(std::io::ErrorKind::InvalidData, err))
        });
    }
    #[cfg(not(feature = "svg"))]
    if looks_like_svg(data) {
        return Err(svg_unsupported_error());
    }

    if let Some((width, height, _)) = probe_dimensions(data) {
        let total_pixels = (width as u64).saturating_mul(height as u64);
        if total_pixels > MAX_DECODE_PIXELS {
            return Err(dimension_limit_error());
        }
    }

    let mut reader = image::ImageReader::new(Cursor::new(data)).with_guessed_format()?;
    reader.limits(decode_limits());
    let format = reader.format().and_then(from_image_format);
    let dynamic = reader.decode()?;
    let rgba = dynamic.into_rgba8();
    let (width, height) = rgba.dimensions();
    Ok(Decoded {
        image: rgba,
        width: width as usize,
        height: height as usize,
        format: format.unwrap_or(Format::Png),
    })
}

pub fn probe_dimensions(data: &[u8]) -> Option<(usize, usize, Format)> {
    let format = sniff_format(data)?;
    let reader = image::ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .ok()?;
    let (w, h) = reader.into_dimensions().ok()?;
    Some((w as usize, h as usize, format))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn encode_png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 200, 30, 255]));
        let mut buf = Vec::new();
        let mut cursor = Cursor::new(&mut buf);
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut cursor, image::ImageFormat::Png)
            .expect("encode test png");
        buf
    }

    #[test]
    fn decode_still_reads_png_dimensions() {
        let data = encode_png(7, 5);
        let decoded = decode_still(&data).expect("decode");
        assert_eq!((decoded.width, decoded.height), (7, 5));
        assert_eq!(decoded.format, Format::Png);
    }

    #[test]
    fn decode_still_rejects_empty_input() {
        assert!(decode_still(&[]).is_err());
    }

    #[test]
    fn decode_still_rejects_garbage() {
        assert!(decode_still(b"not an image at all").is_err());
    }

    #[test]
    fn sniff_format_detects_svg_with_xml_prologue() {
        let svg = br#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg"/>"#;
        assert_eq!(sniff_format(svg), Some(Format::Svg));
    }

    #[test]
    fn sniff_format_detects_png() {
        let data = encode_png(2, 2);
        assert_eq!(sniff_format(&data), Some(Format::Png));
    }

    #[test]
    fn sniff_format_reports_none_for_garbage() {
        assert_eq!(sniff_format(b"zzzz"), None);
    }

    #[test]
    fn probe_dimensions_reads_header_only() {
        let data = encode_png(9, 4);
        let (w, h, format) = probe_dimensions(&data).expect("probe");
        assert_eq!((w, h, format), (9, 4, Format::Png));
    }

    #[test]
    fn extensions_cover_every_still_format() {
        let exts = extensions();
        for ext in ["png", "jpg", "jpeg", "gif", "bmp", "tiff", "webp"] {
            assert!(exts.contains(&ext), "missing extension {ext:?}");
        }
    }

    const WEBP: &[u8] = include_bytes!("../../../testdata/assets/z.webp");

    #[test]
    fn sniff_format_detects_webp() {
        assert_eq!(sniff_format(WEBP), Some(Format::WebP));
    }

    #[test]
    fn decode_still_reads_real_webp_bytes() {
        let decoded = decode_still(WEBP).expect("decode webp");
        assert_eq!((decoded.width, decoded.height), (32, 24));
        assert_eq!(decoded.format, Format::WebP);
        assert_eq!(decoded.image.get_pixel(0, 0).0, [10, 200, 30, 255]);
    }

    fn encode(w: u32, h: u32, format: image::ImageFormat) -> Vec<u8> {
        let img = if format == image::ImageFormat::Jpeg {
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                w,
                h,
                image::Rgb([10, 200, 30]),
            ))
        } else {
            image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                w,
                h,
                image::Rgba([10, 200, 30, 255]),
            ))
        };
        let mut buf = Vec::new();
        let mut cursor = Cursor::new(&mut buf);
        img.write_to(&mut cursor, format)
            .expect("encode test image");
        buf
    }

    #[test]
    fn probe_dimensions_reports_jpeg() {
        let data = encode(6, 3, image::ImageFormat::Jpeg);
        let (w, h, format) = probe_dimensions(&data).expect("probe jpeg");
        assert_eq!((w, h, format), (6, 3, Format::Jpeg));
    }

    #[test]
    fn probe_dimensions_reports_gif() {
        let data = encode(6, 3, image::ImageFormat::Gif);
        let (w, h, format) = probe_dimensions(&data).expect("probe gif");
        assert_eq!((w, h, format), (6, 3, Format::Gif));
    }

    #[test]
    fn probe_dimensions_reports_bmp() {
        let data = encode(6, 3, image::ImageFormat::Bmp);
        let (w, h, format) = probe_dimensions(&data).expect("probe bmp");
        assert_eq!((w, h, format), (6, 3, Format::Bmp));
    }

    #[test]
    fn probe_dimensions_reports_tiff() {
        let data = encode(6, 3, image::ImageFormat::Tiff);
        let (w, h, format) = probe_dimensions(&data).expect("probe tiff");
        assert_eq!((w, h, format), (6, 3, Format::Tiff));
    }

    #[test]
    fn looks_like_svg_accepts_an_xml_prologue_with_no_doctype_or_comment() {
        let data = br#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg"/>"#;
        assert_eq!(sniff_format(data), Some(Format::Svg));
    }

    #[test]
    fn looks_like_svg_accepts_a_doctype_prologue_with_no_xml_or_comment() {
        let data = b"<!doctype svg><svg xmlns=\"http://www.w3.org/2000/svg\"></svg>";
        assert_eq!(sniff_format(data), Some(Format::Svg));
    }

    #[test]
    fn looks_like_svg_accepts_a_leading_comment_with_no_xml_or_doctype() {
        let data = b"<!-- comment --><svg xmlns=\"http://www.w3.org/2000/svg\"></svg>";
        assert_eq!(sniff_format(data), Some(Format::Svg));
    }

    #[test]
    fn extensions_reflect_whether_the_svg_decoder_is_actually_compiled_in() {
        let exts = extensions();
        let has_svg = exts.contains(&"svg");
        assert_eq!(
            has_svg,
            cfg!(feature = "svg"),
            "extensions() must advertise \"svg\" exactly when the svg feature compiles a decoder for it"
        );
    }

    #[test]
    fn looks_like_svg_only_examines_a_bounded_prefix_of_the_input() {
        let mut data = vec![b'x'; SVG_SNIFF_WINDOW * 4];
        data.extend_from_slice(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>");
        assert_eq!(sniff_format(&data), None);
    }

    #[test]
    fn looks_like_svg_still_finds_a_prologue_that_fits_inside_the_window() {
        let mut data = vec![b' '; SVG_SNIFF_WINDOW / 2];
        data.extend_from_slice(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>");
        assert_eq!(sniff_format(&data), Some(Format::Svg));
    }

    #[cfg(not(feature = "svg"))]
    #[test]
    fn decode_still_reports_missing_svg_support_instead_of_an_unknown_format() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"/>"#;
        let err = decode_still(svg).expect_err("svg must fail without the svg feature");
        assert!(err.to_string().contains("svg support not built"));
    }
}
