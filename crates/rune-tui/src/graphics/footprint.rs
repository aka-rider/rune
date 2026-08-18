//! Fit-to-width footprint math for an image document's live pixels:
//! how many terminal columns/rows a decoded image occupies once
//! it is capped to the pane's own width and never upscaled. Pure — no
//! `App`, no `Document`, so both the initial decode
//! (`graphics::decode_cmd::handle_image_decoded`) and a later re-fit on
//! resize (`graphics::resize_refit::refit_on_resize`) compute the SAME
//! `(cols, rows)` for the same inputs, with one implementation.
//!
//! Deliberately distinct from [`rune_image::fit_cells`]: that function fits
//! independently against a `(max_cols, max_rows)` BOX. This one
//! fits to WIDTH ONLY — Decision 8's
//! "fit-to-width, never upscale, vertical scroll" — and derives the row
//! count from whatever the width-driven scale leaves, rather than
//! shrinking further to fit a row cap that doesn't exist here (an image
//! document scrolls vertically instead).

use rune_image::{CellFootprint, CellSize, PixelSize};

/// `cols = min(pane_width, ceil_div(px.w, cell.w))`; the scale that
/// footprint implies (`cols * cell.w / px.w`, capped at `1.0` so a small
/// image is never upscaled to fill the pane); `rows` from applying that
/// scale to `px.h` and ceiling to whole cells. Returns a zero footprint for
/// any degenerate input (a zero pixel dimension, a zero cell dimension, or a
/// zero-width pane) — the caller (`DisplaySnapshot::image_rows`) floors the
/// row count at 1 on its own, so this never needs to.
pub(crate) fn fit(px: PixelSize, pane_width: usize, cell: CellSize) -> CellFootprint {
    if px.w == 0 || px.h == 0 || cell.w == 0 || cell.h == 0 || pane_width == 0 {
        return CellFootprint { cols: 0, rows: 0 };
    }
    let cols = pane_width.min(px.w.div_ceil(cell.w));
    if cols == 0 {
        return CellFootprint { cols: 0, rows: 0 };
    }
    let scale = ((cols * cell.w) as f64 / px.w as f64).min(1.0);
    let rows = ((px.h as f64 * scale) as usize).div_ceil(cell.h);
    CellFootprint { cols, rows }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn fits_to_pane_width_and_derives_rows_from_the_same_scale() {
        // 64x48 px, 8x16 cells, a 20-column pane: natural width is
        // ceil(64/8) = 8 cols, well under the pane, so no scaling at all —
        // rows = ceil(48/16) = 3.
        let footprint = fit(PixelSize { w: 64, h: 48 }, 20, CellSize { w: 8, h: 16 });
        assert_eq!((footprint.cols, footprint.rows), (8, 3));
    }

    #[test]
    fn never_upscales_past_the_pane_width_cap() {
        // A pane narrower than the image's natural footprint clamps cols to
        // the pane, then derives a SMALLER scale for rows.
        let footprint = fit(PixelSize { w: 64, h: 48 }, 4, CellSize { w: 8, h: 16 });
        assert_eq!(footprint.cols, 4);
        // scale = 4*8/64 = 0.5; rows = ceil(48*0.5/16) = ceil(1.5) = 2.
        assert_eq!(footprint.rows, 2);
    }

    #[test]
    fn degenerate_input_yields_zero() {
        let zero = CellFootprint { cols: 0, rows: 0 };
        assert_eq!(
            fit(PixelSize { w: 0, h: 48 }, 20, CellSize { w: 8, h: 16 }),
            zero
        );
        assert_eq!(
            fit(PixelSize { w: 64, h: 48 }, 20, CellSize { w: 0, h: 16 }),
            zero
        );
        assert_eq!(
            fit(PixelSize { w: 64, h: 48 }, 0, CellSize { w: 8, h: 16 }),
            zero
        );
    }
}
