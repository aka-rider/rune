use crate::decode::Decoded;
use crate::decode::Format;

const MAX_AXIS: u32 = 4096;

fn parse_options() -> usvg::Options<'static> {
    let mut opt = usvg::Options::default();
    opt.image_href_resolver.resolve_string = Box::new(|_href: &str, _opt: &usvg::Options| None);
    opt
}

pub fn decode_svg(data: &[u8]) -> Result<Decoded, SvgError> {
    // usvg::Tree::from_data hardcodes roxmltree's allow_dtd to true with no
    // entity_resolver, so internal entities do resolve; roxmltree's own
    // LoopDetector (10 levels deep, 255 references per level) is what
    // actually stops a billion-laughs-shaped input, not anything configured
    // here.
    let opt = parse_options();
    let tree = usvg::Tree::from_data(data, &opt).map_err(SvgError::Parse)?;

    let size = tree.size();
    let width = (size.width().ceil() as u32).clamp(1, MAX_AXIS);
    let height = (size.height().ceil() as u32).clamp(1, MAX_AXIS);

    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height).ok_or(SvgError::EmptyCanvas)?;

    let scale_x = width as f32 / size.width();
    let scale_y = height as f32 / size.height();
    let transform = resvg::tiny_skia::Transform::from_scale(scale_x, scale_y);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    let rgba_bytes = pixmap.take_demultiplied();
    let image = image::RgbaImage::from_raw(width, height, rgba_bytes).ok_or(SvgError::Rasterize)?;

    Ok(Decoded {
        image,
        width: width as usize,
        height: height as usize,
        format: Format::Svg,
    })
}

#[derive(Debug)]
pub enum SvgError {
    Parse(usvg::Error),
    EmptyCanvas,
    Rasterize,
}

impl std::fmt::Display for SvgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SvgError::Parse(err) => write!(f, "parse svg: {err}"),
            SvgError::EmptyCanvas => write!(f, "svg rasterized to an empty canvas"),
            SvgError::Rasterize => write!(f, "svg rasterization produced no pixels"),
        }
    }
}

impl std::error::Error for SvgError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SvgError::Parse(err) => Some(err),
            SvgError::EmptyCanvas | SvgError::Rasterize => None,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    const VECTOR_SVG: &[u8] = include_bytes!("../../../testdata/assets/vector.svg");

    #[test]
    fn decode_svg_reads_declared_view_box_size() {
        let decoded = decode_svg(VECTOR_SVG).expect("decode svg");
        assert_eq!((decoded.width, decoded.height), (64, 48));
        assert_eq!(decoded.format, Format::Svg);
    }

    #[test]
    fn decode_svg_paints_the_circle_at_centre() {
        let decoded = decode_svg(VECTOR_SVG).expect("decode svg");
        let pixel = decoded.image.get_pixel(32, 24);
        // #ffcc00 circle fill.
        assert_eq!(pixel.0, [0xff, 0xcc, 0x00, 0xff]);
    }

    #[test]
    fn decode_svg_paints_the_rect_at_a_corner() {
        let decoded = decode_svg(VECTOR_SVG).expect("decode svg");
        let pixel = decoded.image.get_pixel(2, 2);
        // #6633cc rect fill.
        assert_eq!(pixel.0, [0x66, 0x33, 0xcc, 0xff]);
    }

    #[test]
    fn decode_svg_rejects_garbage() {
        assert!(decode_svg(b"not an svg document").is_err());
    }

    #[test]
    fn every_variant_display_message_is_non_empty() {
        assert!(!SvgError::EmptyCanvas.to_string().is_empty());
        assert!(!SvgError::Rasterize.to_string().is_empty());
        let err = decode_svg(b"not an svg document").expect_err("garbage must fail to parse");
        assert!(matches!(err, SvgError::Parse(_)));
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn parse_error_variant_exposes_its_inner_error_as_the_source() {
        let err = decode_svg(b"not an svg document").expect_err("garbage must fail to parse");
        assert!(std::error::Error::source(&err).is_some());
    }

    fn write_real_png_fixture() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "rune-image-svg-href-refusal-{}-{}.png",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]));
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .expect("encode fixture png");
        std::fs::write(&path, &buf).expect("write fixture png");
        path
    }

    #[test]
    fn our_string_resolver_refuses_a_file_the_upstream_default_would_have_read() {
        let path = write_real_png_fixture();
        let href = path.to_string_lossy().into_owned();

        let default_opt = usvg::Options::default();
        let via_default = (default_opt.image_href_resolver.resolve_string)(&href, &default_opt);
        assert!(
            via_default.is_some(),
            "sanity: the upstream default resolver does read local files by href"
        );

        let our_opt = parse_options();
        let via_ours = (our_opt.image_href_resolver.resolve_string)(&href, &our_opt);
        assert!(
            via_ours.is_none(),
            "our resolver must refuse the same href instead of reading the file"
        );

        std::fs::remove_file(&path).ok();
    }

    const BILLION_LAUGHS_SVG: &[u8] = br#"<?xml version="1.0"?>
<!DOCTYPE svg [
<!ENTITY lol "lol">
<!ENTITY lol1 "&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;">
<!ENTITY lol2 "&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;">
<!ENTITY lol3 "&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;">
<!ENTITY lol4 "&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;">
<!ENTITY lol5 "&lol4;&lol4;&lol4;&lol4;&lol4;&lol4;&lol4;&lol4;&lol4;&lol4;">
<!ENTITY lol6 "&lol5;&lol5;&lol5;&lol5;&lol5;&lol5;&lol5;&lol5;&lol5;&lol5;">
<!ENTITY lol7 "&lol6;&lol6;&lol6;&lol6;&lol6;&lol6;&lol6;&lol6;&lol6;&lol6;">
<!ENTITY lol8 "&lol7;&lol7;&lol7;&lol7;&lol7;&lol7;&lol7;&lol7;&lol7;&lol7;">
<!ENTITY lol9 "&lol8;&lol8;&lol8;&lol8;&lol8;&lol8;&lol8;&lol8;&lol8;&lol8;">
]>
<svg xmlns="http://www.w3.org/2000/svg"><text>&lol9;</text></svg>
"#;

    #[test]
    fn a_billion_laughs_shaped_svg_is_refused_instead_of_expanded() {
        let err = decode_svg(BILLION_LAUGHS_SVG).expect_err("entity blowup must not parse");
        assert!(matches!(err, SvgError::Parse(_)));
    }
}
