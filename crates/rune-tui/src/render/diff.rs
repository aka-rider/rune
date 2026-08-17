use std::ops::Range;

use ratatui::style::Style;

use rune_merge::{AlignmentMap, Region, RegionKind};
use rune_syntax::wrap::WrapSnapshot;

use crate::diff_view::DiffView;
use crate::diff_view::rows::{self, FoldSlot, RowLayout, RowSlot, Side};

use super::cell::{Cell, paint_range};
use super::overlay;

pub(super) fn layout(diff: &DiffView, other_wrap: &WrapSnapshot) -> RowLayout {
    let left_heights = diff
        .left
        .view
        .as_ref()
        .map(|v| rows::line_heights(&v.wrap))
        .unwrap_or_default();
    let right_heights = rows::line_heights(other_wrap);
    rows::layout_rows(&diff.alignment, &left_heights, &right_heights)
}

pub(super) fn augment(
    native_rows: &[Vec<Cell>],
    layout: &RowLayout,
    side: Side,
    native_scroll: usize,
    height: u16,
    width: u16,
) -> Vec<Vec<Cell>> {
    let plan = rows::plan_side(layout, side, native_scroll, height as usize);
    plan.iter()
        .map(|slot| match slot {
            RowSlot::Content(idx) => native_rows
                .get(idx - native_scroll)
                .cloned()
                .unwrap_or_else(|| filler_row(width)),
            RowSlot::Filler => filler_row(width),
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub(super) fn augment_fold(
    right_rows: &[Vec<Cell>],
    left_rows: &[Vec<Cell>],
    layout: &RowLayout,
    right_native_scroll: usize,
    left_native_scroll: usize,
    theirs_bg: Style,
    height: u16,
    width: u16,
) -> Vec<Vec<Cell>> {
    let plan = rows::plan_fold(layout, right_native_scroll, height as usize);
    plan.iter()
        .map(|slot| match slot {
            FoldSlot::Right(idx) => right_rows
                .get(idx - right_native_scroll)
                .cloned()
                .unwrap_or_else(|| filler_row(width)),
            FoldSlot::LeftVirtual(idx) => {
                let mut row = left_rows
                    .get(idx - left_native_scroll)
                    .cloned()
                    .unwrap_or_else(|| filler_row(width));
                mark_virtual(&mut row, theirs_bg);
                row
            }
        })
        .collect()
}

fn mark_virtual(row: &mut [Cell], theirs_bg: Style) {
    for cell in row.iter_mut() {
        cell.buf_offset = -1;
        cell.style = cell.style.patch(theirs_bg);
    }
}

fn filler_row(width: u16) -> Vec<Cell> {
    (0..width)
        .map(|_| Cell {
            text: " ".into(),
            width: 1,
            style: Style::default(),
            buf_offset: -1,
        })
        .collect()
}

pub(super) fn paint_backgrounds(
    rows: &mut [Vec<Cell>],
    alignment: &AlignmentMap,
    content: &str,
    side_lines: impl Fn(&Region) -> Range<usize>,
    include: impl Fn(RegionKind) -> bool,
    style: Style,
) {
    let Some(visible) = overlay::visible_byte_range(rows) else {
        return;
    };
    for region in &alignment.regions {
        if !include(region.kind) {
            continue;
        }
        let range = rows::line_byte_range(content, side_lines(region));
        if range.start >= visible.end || range.end <= visible.start {
            continue;
        }
        paint_range(rows, range, style);
    }
}

pub(super) fn paint_intraline(rows: &mut [Vec<Cell>], ranges: &[Range<usize>], style: Style) {
    for range in ranges {
        paint_range(rows, range.clone(), style);
    }
}
