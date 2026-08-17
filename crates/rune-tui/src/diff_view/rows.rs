use std::ops::Range;

use rune_core::buffer::Buffer;
use rune_merge::{AlignmentMap, RegionKind, line_starts};
use rune_syntax::wrap::WrapSnapshot;

pub fn line_offset(buffer: &Buffer, line: usize) -> usize {
    if line >= buffer.line_count() {
        buffer.len()
    } else {
        buffer.line_start(line).unwrap_or_else(|| buffer.len())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionLayout {
    pub kind: RegionKind,
    pub left_start: usize,
    pub left_rows: usize,
    pub right_start: usize,
    pub right_rows: usize,
    pub height: usize,
}

#[derive(Debug, Clone, Default)]
pub struct RowLayout {
    pub regions: Vec<RegionLayout>,
    pub left_total: usize,
    pub right_total: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    fn start_rows(self, region: &RegionLayout) -> (usize, usize) {
        match self {
            Side::Left => (region.left_start, region.left_rows),
            Side::Right => (region.right_start, region.right_rows),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowSlot {
    Content(usize),
    Filler,
}

pub fn line_heights(wrap: &WrapSnapshot) -> Vec<usize> {
    let mut heights: Vec<usize> = Vec::new();
    for seg in wrap.segments() {
        if seg.model_line >= heights.len() {
            heights.resize(seg.model_line + 1, 0);
        }
        if let Some(h) = heights.get_mut(seg.model_line) {
            *h += 1;
        }
    }
    heights
}

fn sum_heights(heights: &[usize], lines: Range<usize>) -> usize {
    lines.map(|i| heights.get(i).copied().unwrap_or(1)).sum()
}

pub fn line_byte_range(content: &str, lines: Range<usize>) -> Range<usize> {
    let starts = line_starts(content);
    let start = starts.get(lines.start).copied().unwrap_or(content.len());
    let end = starts.get(lines.end).copied().unwrap_or(content.len());
    start..end
}

pub fn region_text(content: &str, lines: Range<usize>) -> (usize, String) {
    let range = line_byte_range(content, lines);
    (
        range.start,
        content.get(range).unwrap_or_default().to_string(),
    )
}

pub fn layout_rows(
    alignment: &AlignmentMap,
    left_heights: &[usize],
    right_heights: &[usize],
) -> RowLayout {
    let mut regions = Vec::with_capacity(alignment.regions.len());
    let mut left_start = 0usize;
    let mut right_start = 0usize;
    for region in &alignment.regions {
        let left_rows = sum_heights(left_heights, region.left_lines.clone());
        let right_rows = sum_heights(right_heights, region.right_lines.clone());
        let height = left_rows.max(right_rows);
        regions.push(RegionLayout {
            kind: region.kind,
            left_start,
            left_rows,
            right_start,
            right_rows,
            height,
        });
        left_start += left_rows;
        right_start += right_rows;
    }
    RowLayout {
        regions,
        left_total: left_start,
        right_total: right_start,
    }
}

pub fn left_row_for_right_row(layout: &RowLayout, right_row: usize) -> usize {
    for region in &layout.regions {
        let end = region.right_start + region.right_rows;
        if region.right_rows > 0 && right_row < end {
            let p = right_row - region.right_start;
            return region.left_start + p.min(region.left_rows.saturating_sub(1));
        }
    }
    layout.left_total.saturating_sub(1)
}

pub fn right_line_for_left_line(alignment: &AlignmentMap, left_line: usize) -> usize {
    for region in &alignment.regions {
        if region.left_lines.contains(&left_line) {
            if region.right_lines.is_empty() {
                return region.right_lines.start;
            }
            let offset = left_line - region.left_lines.start;
            let idx = offset.min(region.right_lines.len().saturating_sub(1));
            return region.right_lines.start + idx;
        }
    }
    alignment
        .regions
        .last()
        .map_or(0, |region| region.right_lines.end.saturating_sub(1))
}

fn locate(layout: &RowLayout, side: Side, native_row: usize) -> Option<(usize, usize)> {
    if layout.regions.is_empty() {
        return None;
    }
    if native_row == 0 {
        return Some((0, 0));
    }
    for (idx, region) in layout.regions.iter().enumerate() {
        let (start, rows) = side.start_rows(region);
        if rows > 0 && native_row < start + rows {
            return Some((idx, native_row - start));
        }
    }
    None
}

pub fn plan_side(
    layout: &RowLayout,
    side: Side,
    native_start: usize,
    height: usize,
) -> Vec<RowSlot> {
    let Some((mut region_idx, mut p)) = locate(layout, side, native_start) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(height);
    let mut content_idx = native_start;
    while out.len() < height {
        let Some(region) = layout.regions.get(region_idx) else {
            break;
        };
        let (_, side_rows) = side.start_rows(region);
        if p < side_rows {
            out.push(RowSlot::Content(content_idx));
            content_idx += 1;
        } else {
            out.push(RowSlot::Filler);
        }
        p += 1;
        if p >= region.height {
            region_idx += 1;
            p = 0;
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use std::ops::Range as StdRange;

    fn region(
        kind: RegionKind,
        left: StdRange<usize>,
        right: StdRange<usize>,
    ) -> rune_merge::Region {
        rune_merge::Region {
            kind,
            left_lines: left,
            right_lines: right,
        }
    }

    #[test]
    fn equal_heights_produce_direct_correspondence() {
        let alignment = AlignmentMap {
            regions: vec![region(RegionKind::Same, 0..3, 0..3)],
        };
        let left_heights = vec![1, 1, 1];
        let right_heights = vec![1, 1, 1];
        let layout = layout_rows(&alignment, &left_heights, &right_heights);
        assert_eq!(layout.regions[0].height, 3);
        for row in 0..3 {
            assert_eq!(left_row_for_right_row(&layout, row), row);
        }
    }

    #[test]
    fn unequal_wrapped_heights_pad_the_shorter_side() {
        let alignment = AlignmentMap {
            regions: vec![region(RegionKind::Changed, 0..1, 0..1)],
        };
        let left_heights = vec![3];
        let right_heights = vec![1];
        let layout = layout_rows(&alignment, &left_heights, &right_heights);
        assert_eq!(layout.regions[0].left_rows, 3);
        assert_eq!(layout.regions[0].right_rows, 1);
        assert_eq!(layout.regions[0].height, 3);

        let plan = plan_side(&layout, Side::Right, 0, 3);
        assert_eq!(
            plan,
            vec![RowSlot::Content(0), RowSlot::Filler, RowSlot::Filler]
        );
        let plan = plan_side(&layout, Side::Left, 0, 3);
        assert_eq!(
            plan,
            vec![
                RowSlot::Content(0),
                RowSlot::Content(1),
                RowSlot::Content(2)
            ]
        );
    }

    #[test]
    fn left_only_region_shows_as_filler_on_the_right() {
        let alignment = AlignmentMap {
            regions: vec![
                region(RegionKind::LeftOnly, 0..2, 0..0),
                region(RegionKind::Same, 2..3, 0..1),
            ],
        };
        let left_heights = vec![1, 1, 1];
        let right_heights = vec![1];
        let layout = layout_rows(&alignment, &left_heights, &right_heights);
        assert_eq!(layout.regions[0].height, 2);
        assert_eq!(layout.regions[0].right_rows, 0);

        assert_eq!(left_row_for_right_row(&layout, 0), 2);

        let plan = plan_side(&layout, Side::Right, 0, 3);
        assert_eq!(
            plan,
            vec![RowSlot::Filler, RowSlot::Filler, RowSlot::Content(0)]
        );
    }

    #[test]
    fn right_only_region_shows_as_filler_on_the_left() {
        let alignment = AlignmentMap {
            regions: vec![region(RegionKind::RightOnly, 0..0, 0..2)],
        };
        let left_heights: Vec<usize> = vec![];
        let right_heights = vec![1, 1];
        let layout = layout_rows(&alignment, &left_heights, &right_heights);
        assert_eq!(layout.regions[0].height, 2);
        assert_eq!(layout.regions[0].left_rows, 0);

        assert_eq!(left_row_for_right_row(&layout, 0), 0);
        assert_eq!(left_row_for_right_row(&layout, 1), 0);

        let plan = plan_side(&layout, Side::Left, 0, 2);
        assert_eq!(plan, vec![RowSlot::Filler, RowSlot::Filler]);
    }

    #[test]
    fn empty_files_produce_no_regions_and_no_rows() {
        let alignment = AlignmentMap { regions: vec![] };
        let layout = layout_rows(&alignment, &[], &[]);
        assert!(layout.regions.is_empty());
        assert_eq!(layout.left_total, 0);
        assert_eq!(layout.right_total, 0);
        assert_eq!(left_row_for_right_row(&layout, 0), 0);
        assert_eq!(plan_side(&layout, Side::Right, 0, 5), Vec::new());
    }

    #[test]
    fn line_heights_counts_wrap_segments_per_source_line() {
        use rune_syntax::syntax::SyntaxLine;
        use rune_syntax::wrap::WrapMap;

        let content = "a\nbb\ncccccccccc\n";
        let lines: Vec<SyntaxLine> = content.lines().map(|_| SyntaxLine::default()).collect();
        let wrap = WrapMap::new(3).sync(content, &lines);
        let heights = line_heights(&wrap);
        assert_eq!(heights.len(), 3);
        assert_eq!(heights[0], 1);
    }
}
