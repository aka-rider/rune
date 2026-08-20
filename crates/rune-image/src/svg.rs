use crate::decode::Decoded;
use crate::decode::Format;

const MAX_AXIS: u32 = 4096;

pub fn decode_svg(data: &[u8]) -> Result<Decoded, SvgError> {
    let opt = usvg::Options::default();
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
}
