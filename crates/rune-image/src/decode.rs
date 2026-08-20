use std::io::Cursor;

pub use image::ImageError;

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
    let Ok(text) = std::str::from_utf8(without_bom) else {
        return false;
    };
    let trimmed = text.trim();
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

    let reader = image::ImageReader::new(Cursor::new(data)).with_guessed_format()?;
    let format = reader.format().and_then(from_image_format);
    let dynamic = reader.decode()?;
    let rgba = dynamic.to_rgba8();
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
}
