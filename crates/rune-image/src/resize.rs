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
