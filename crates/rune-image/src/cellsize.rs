//! Terminal cell pixel geometry and the "fit an image into a cell box" math.

/// The pixel size of a single terminal cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellSize {
    pub w: usize,
    pub h: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelSize {
    pub w: usize,
    pub h: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellFootprint {
    pub cols: usize,
    pub rows: usize,
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

/// Fits `px` into `max` terminal cells, preserving aspect ratio. Both
/// results are at least 1 for a non-degenerate image.
pub fn fit_cells(px: PixelSize, max: CellFootprint, cs: CellSize) -> CellFootprint {
    if px.w == 0 || px.h == 0 || cs.w == 0 || cs.h == 0 {
        return CellFootprint { cols: 0, rows: 0 };
    }
    let max_cols = max.cols.max(1);
    let max_rows = max.rows.max(1);

    // Natural cell footprint, rounding up so the image is never clipped.
    let mut cols = ceil_div(px.w, cs.w);
    let mut rows = ceil_div(px.h, cs.h);

    // Scale down preserving aspect if it exceeds the allowed box.
    if cols > max_cols || rows > max_rows {
        let sw = max_cols as f64 / cols as f64;
        let sh = max_rows as f64 / rows as f64;
        let s = sw.min(sh);
        // Truncating cast: the scaled dimension rounds toward zero.
        cols = (cols as f64 * s) as usize;
        rows = (rows as f64 * s) as usize;
    }

    CellFootprint {
        cols: cols.max(1),
        rows: rows.max(1),
    }
}
