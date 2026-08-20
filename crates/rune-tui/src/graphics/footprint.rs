use rune_image::{CellFootprint, CellSize, PixelSize};

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
        let footprint = fit(PixelSize { w: 64, h: 48 }, 20, CellSize { w: 8, h: 16 });
        assert_eq!((footprint.cols, footprint.rows), (8, 3));
    }

    #[test]
    fn never_upscales_past_the_pane_width_cap() {
        let footprint = fit(PixelSize { w: 64, h: 48 }, 4, CellSize { w: 8, h: 16 });
        assert_eq!(footprint.cols, 4);
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
