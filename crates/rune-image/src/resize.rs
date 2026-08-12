//! Fitting a source pixel box into a destination pixel box (`fit_box`), and
//! resampling pixels to an exact target size.

use crate::cellsize::PixelSize;

/// Returns the largest size that preserves `src`'s aspect ratio and fits
/// within `max`. Never upscales beyond `src`, and never returns a dimension
/// below 1 for a non-degenerate source.
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

/// Scales `src` to exactly `w` x `h` using a high-quality CatmullRom
/// kernel. A non-positive `w`/`h` yields a 1x1 image rather than
/// propagating a degenerate size into the resampler.
pub fn resize(src: &image::RgbaImage, w: usize, h: usize) -> image::RgbaImage {
    if w == 0 || h == 0 {
        return image::RgbaImage::new(1, 1);
    }
    let (Ok(nw), Ok(nh)) = (u32::try_from(w), u32::try_from(h)) else {
        return image::RgbaImage::new(1, 1);
    };
    image::imageops::resize(src, nw, nh, image::imageops::FilterType::CatmullRom)
}
