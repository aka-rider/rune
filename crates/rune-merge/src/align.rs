use std::ops::Range;
use std::time::Instant;

use similar::{ChangeTag, DiffTag, TextDiff};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionKind {
    Same,
    Changed,
    LeftOnly,
    RightOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    pub kind: RegionKind,
    pub left_lines: Range<usize>,
    pub right_lines: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlignmentMap {
    pub regions: Vec<Region>,
}

pub fn align(left: &str, right: &str) -> AlignmentMap {
    let diff = TextDiff::from_lines(left, right);
    let regions = diff
        .ops()
        .iter()
        .map(|op| {
            let kind = match op.tag() {
                DiffTag::Equal => RegionKind::Same,
                DiffTag::Delete => RegionKind::LeftOnly,
                DiffTag::Insert => RegionKind::RightOnly,
                DiffTag::Replace => RegionKind::Changed,
            };
            Region {
                kind,
                left_lines: op.old_range(),
                right_lines: op.new_range(),
            }
        })
        .collect();
    AlignmentMap { regions }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineSpans {
    pub line: usize,
    pub ranges: Vec<Range<usize>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IntralineSpans {
    pub left: Vec<LineSpans>,
    pub right: Vec<LineSpans>,
}

pub fn intraline(left: &str, right: &str, deadline: Option<Instant>) -> IntralineSpans {
    let diff = TextDiff::from_lines(left, right);
    let mut spans = IntralineSpans::default();

    for op in diff.ops().iter().filter(|op| op.tag() == DiffTag::Replace) {
        for change in diff.iter_inline_changes_deadline(op, deadline) {
            let ranges = emphasized_ranges(change.values());
            if ranges.is_empty() {
                continue;
            }
            match (change.tag(), change.old_index(), change.new_index()) {
                (ChangeTag::Delete, Some(line), _) => {
                    spans.left.push(LineSpans { line, ranges });
                }
                (ChangeTag::Insert, _, Some(line)) => {
                    spans.right.push(LineSpans { line, ranges });
                }
                _ => {}
            }
        }
    }

    spans
}

fn emphasized_ranges(values: &[(bool, &str)]) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut offset = 0usize;
    for (emphasized, value) in values {
        let len = value.len();
        if *emphasized {
            ranges.push(offset..offset + len);
        }
        offset += len;
    }
    if ranges.is_empty() && offset > 0 {
        ranges.push(0..offset);
    }
    ranges
}
