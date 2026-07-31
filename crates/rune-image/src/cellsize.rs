//! Terminal cell pixel geometry and the "fit an image into a cell box" math.

/// The pixel size of a single terminal cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellSize {
    pub w: usize,
    pub h: usize,
}

/// The conventional 8x16 fallback used when the terminal does not report its
/// cell pixel dimensions. Aspect ratio may be slightly off on a terminal with
/// a different cell geometry, but rendering is never broken.
pub const DEFAULT_CELL_SIZE: CellSize = CellSize { w: 8, h: 16 };

/// Ceiling division; `0` when the divisor is `0`, matching the reference
/// implementation's degenerate-input behaviour.
fn ceil_div(a: usize, b: usize) -> usize {
    if b == 0 { 0 } else { a.div_ceil(b) }
}

/// Computes how many terminal columns and rows an image of `px_w` x `px_h`
/// pixels should occupy, preserving aspect ratio and fitting within
/// `max_cols` x `max_rows`. Both results are at least 1 for a non-degenerate
/// image.
pub fn fit_cells(
    px_w: usize,
    px_h: usize,
    max_cols: usize,
    max_rows: usize,
    cs: CellSize,
) -> (usize, usize) {
    if px_w == 0 || px_h == 0 || cs.w == 0 || cs.h == 0 {
        return (0, 0);
    }
    let max_cols = max_cols.max(1);
    let max_rows = max_rows.max(1);

    // Natural cell footprint, rounding up so the image is never clipped.
    let mut cols = ceil_div(px_w, cs.w);
    let mut rows = ceil_div(px_h, cs.h);

    // Scale down preserving aspect if it exceeds the allowed box.
    if cols > max_cols || rows > max_rows {
        let sw = max_cols as f64 / cols as f64;
        let sh = max_rows as f64 / rows as f64;
        let s = sw.min(sh);
        // Truncating cast, matching the reference implementation's `int(...)`.
        cols = (cols as f64 * s) as usize;
        rows = (rows as f64 * s) as usize;
    }

    (cols.max(1), rows.max(1))
}
