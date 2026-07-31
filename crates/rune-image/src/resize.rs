//! Fitting a source pixel box into a destination pixel box (`fit_box`), and
//! resampling pixels to an exact target size.

/// Returns the largest `(w, h)` that preserves the source aspect ratio and
/// fits within `max_w` x `max_h`. Never upscales beyond the source
/// dimensions, and never returns a dimension below 1 for a non-degenerate
/// source.
pub fn fit_box(src_w: usize, src_h: usize, max_w: usize, max_h: usize) -> (usize, usize) {
    if src_w == 0 || src_h == 0 || max_w == 0 || max_h == 0 {
        return (0, 0);
    }
    if src_w <= max_w && src_h <= max_h {
        return (src_w, src_h);
    }
    let sw = max_w as f64 / src_w as f64;
    let sh = max_h as f64 / src_h as f64;
    let s = sw.min(sh);
    let w = (src_w as f64 * s) as usize;
    let h = (src_h as f64 * s) as usize;
    (w.max(1), h.max(1))
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
