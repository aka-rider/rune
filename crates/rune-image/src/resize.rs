use crate::cellsize::PixelSize;

pub fn fit_box(src: PixelSize, max: PixelSize) -> PixelSize {
    if src.w == 0 || src.h == 0 || max.w == 0 || max.h == 0 {
        return PixelSize { w: 0, h: 0 };
    }
    if src.w <= max.w && src.h <= max.h {
        return src;
    }
    let sw = max.w as f64 / src.w as f64;
    let sh = max.h as f64 / src.h as f64;
    let s = sw.min(sh);
    let w = (src.w as f64 * s) as usize;
    let h = (src.h as f64 * s) as usize;
    PixelSize {
        w: w.max(1),
        h: h.max(1),
    }
}

pub fn resize(src: &image::RgbaImage, w: usize, h: usize) -> image::RgbaImage {
    if w == 0 || h == 0 {
        return image::RgbaImage::new(1, 1);
    }
    let (Ok(nw), Ok(nh)) = (u32::try_from(w), u32::try_from(h)) else {
        return image::RgbaImage::new(1, 1);
    };
    image::imageops::resize(src, nw, nh, image::imageops::FilterType::CatmullRom)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_width_source_yields_an_empty_box() {
        let src = PixelSize { w: 0, h: 10 };
        let max = PixelSize { w: 100, h: 100 };
        assert_eq!(fit_box(src, max), PixelSize { w: 0, h: 0 });
    }

    #[test]
    fn zero_height_source_yields_an_empty_box() {
        let src = PixelSize { w: 10, h: 0 };
        let max = PixelSize { w: 100, h: 100 };
        assert_eq!(fit_box(src, max), PixelSize { w: 0, h: 0 });
    }

    #[test]
    fn zero_width_max_yields_an_empty_box() {
        let src = PixelSize { w: 10, h: 10 };
        let max = PixelSize { w: 0, h: 100 };
        assert_eq!(fit_box(src, max), PixelSize { w: 0, h: 0 });
    }

    #[test]
    fn zero_height_max_yields_an_empty_box() {
        let src = PixelSize { w: 10, h: 10 };
        let max = PixelSize { w: 100, h: 0 };
        assert_eq!(fit_box(src, max), PixelSize { w: 0, h: 0 });
    }

    #[test]
    fn resize_to_zero_width_produces_a_single_pixel_placeholder() {
        let src = image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]));
        let out = resize(&src, 0, 5);
        assert_eq!((out.width(), out.height()), (1, 1));
    }

    #[test]
    fn resize_to_zero_height_produces_a_single_pixel_placeholder() {
        let src = image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]));
        let out = resize(&src, 5, 0);
        assert_eq!((out.width(), out.height()), (1, 1));
    }
}
