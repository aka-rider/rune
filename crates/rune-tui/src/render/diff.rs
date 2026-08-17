use std::ops::Range;

use ratatui::style::{Modifier, Style};

use rune_merge::{AlignmentMap, Region, RegionKind};
use rune_syntax::wrap::WrapSnapshot;

use crate::diff_view::DiffView;
use crate::diff_view::rows::{self, RowLayout, RowSlot, Side};

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

fn filler_row(width: u16) -> Vec<Cell> {
    let style = Style::default().add_modifier(Modifier::DIM);
    (0..width)
        .map(|_| Cell {
            text: "╌".into(),
            width: 1,
            style,
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
        let range = line_byte_range(content, side_lines(region));
        if range.start >= visible.end || range.end <= visible.start {
            continue;
        }
        paint_range(rows, range, style);
    }
}

fn line_byte_range(content: &str, lines: Range<usize>) -> Range<usize> {
    let mut offset = 0usize;
    let mut start = content.len();
    for (idx, line) in content.split_inclusive('\n').enumerate() {
        if idx == lines.start {
            start = offset;
        }
        if idx == lines.end {
            return start..offset;
        }
        offset += line.len();
    }
    start..content.len()
}
