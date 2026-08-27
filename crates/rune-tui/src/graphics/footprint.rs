use rune_image::{CellFootprint, CellSize, PixelSize};

/// A fitted footprint, plus whether the natural fit wanted more rows than
/// `rune_image::ADDRESSABLE_ROWS` can address — a tall image capped at that
/// ceiling loses its bottom rows rather than misrendering them as repeats
/// of row 0, and a caller that cares can surface the loss instead of
/// letting it pass silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fit {
    pub cells: CellFootprint,
    pub truncated: bool,
}

pub(crate) fn fit(px: PixelSize, pane_width: usize, cell: CellSize) -> Fit {
    if px.w == 0 || px.h == 0 || cell.w == 0 || cell.h == 0 || pane_width == 0 {
        return Fit {
            cells: CellFootprint { cols: 0, rows: 0 },
            truncated: false,
        };
    }
    let cols = pane_width.min(px.w.div_ceil(cell.w));
    if cols == 0 {
        return Fit {
            cells: CellFootprint { cols: 0, rows: 0 },
            truncated: false,
        };
    }
    let scale = ((cols * cell.w) as f64 / px.w as f64).min(1.0);
    let rows = ((px.h as f64 * scale) as usize).div_ceil(cell.h);
    let capped_rows = rows.min(rune_image::ADDRESSABLE_ROWS);
    Fit {
        cells: CellFootprint {
            cols,
            rows: capped_rows,
        },
        truncated: capped_rows < rows,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn fits_to_pane_width_and_derives_rows_from_the_same_scale() {
        let fit = fit(PixelSize { w: 64, h: 48 }, 20, CellSize { w: 8, h: 16 });
        assert_eq!((fit.cells.cols, fit.cells.rows), (8, 3));
        assert!(!fit.truncated);
    }

    #[test]
    fn never_upscales_past_the_pane_width_cap() {
        let fit = fit(PixelSize { w: 64, h: 48 }, 4, CellSize { w: 8, h: 16 });
        assert_eq!(fit.cells.cols, 4);
        assert_eq!(fit.cells.rows, 2);
    }

    #[test]
    fn a_tall_images_footprint_never_exceeds_addressable_rows_and_reports_truncation() {
        // A 1080x9000 portrait screenshot in an 80-column pane: the naive
        // fit math wants far more rows than the Kitty placeholder protocol
        // can address (`rune_image::ADDRESSABLE_ROWS`) — a row past that
        // silently repeats `DIACRITICS[0]`'s content instead of its own.
        let fit = fit(PixelSize { w: 1080, h: 9000 }, 80, CellSize { w: 8, h: 16 });
        assert!(
            fit.cells.rows <= rune_image::ADDRESSABLE_ROWS,
            "fit.cells.rows = {} exceeds the {} addressable rows",
            fit.cells.rows,
            rune_image::ADDRESSABLE_ROWS
        );
        assert!(fit.truncated, "a capped footprint must report the loss");
    }

    #[test]
    fn a_footprint_within_addressable_rows_never_reports_truncation() {
        let fit = fit(PixelSize { w: 64, h: 48 }, 20, CellSize { w: 8, h: 16 });
        assert!(!fit.truncated);
    }

    #[test]
    fn degenerate_input_yields_zero_and_no_truncation() {
        let zero = CellFootprint { cols: 0, rows: 0 };
        for fit in [
            fit(PixelSize { w: 0, h: 48 }, 20, CellSize { w: 8, h: 16 }),
            fit(PixelSize { w: 64, h: 48 }, 20, CellSize { w: 0, h: 16 }),
            fit(PixelSize { w: 64, h: 48 }, 0, CellSize { w: 8, h: 16 }),
        ] {
            assert_eq!(fit.cells, zero);
            assert!(!fit.truncated);
        }
    }
}
