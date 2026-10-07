use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::ops::Range;

use rune_core::buffer::{Buffer, Edit};
use rune_md::continuation::{
    self, MAX_ORDERED, MarkerKind, NumberEdit, Prefix, format_number, parse_prefix,
};

pub(super) struct Insertion {
    slot: usize,
    caret_line: usize,
    anchor_line: usize,
    anchor: Prefix,
    number: Range<usize>,
    base: u32,
    width: usize,
}

impl Insertion {
    pub(super) fn of(
        slot: usize,
        caret_line: usize,
        anchor_line: usize,
        anchor: Prefix,
        insert: &str,
        skip: usize,
    ) -> Option<Self> {
        let marker = parse_prefix(insert.get(skip..)?).marker?;
        let MarkerKind::Ordered { n, width, .. } = marker.kind else {
            return None;
        };
        let start = skip + marker.symbol.start;
        Some(Insertion {
            slot,
            caret_line,
            anchor_line,
            anchor,
            number: start..start + width,
            base: n,
            width,
        })
    }

    pub(super) fn slot(&self) -> usize {
        self.slot
    }

    pub(super) fn renumber(&self, insert: &mut String, shift: u32) {
        let number = self.base.saturating_add(shift).min(MAX_ORDERED);
        insert.replace_range(self.number.clone(), &format_number(number, self.width));
    }
}

pub(super) struct Claims {
    spans: Vec<Range<usize>>,
    reach: Vec<usize>,
}

impl Claims {
    pub(super) fn of<'a>(edits: impl IntoIterator<Item = &'a Edit>) -> Self {
        let mut spans: Vec<Range<usize>> = edits.into_iter().map(|e| e.start..e.end).collect();
        spans.sort_unstable_by_key(|span| span.start);
        let mut furthest = 0;
        let reach = spans
            .iter()
            .map(|span| {
                furthest = furthest.max(span.end);
                furthest
            })
            .collect();
        Claims { spans, reach }
    }

    fn overlaps(&self, range: &Range<usize>) -> bool {
        let starting_before_end = self.spans.partition_point(|span| span.start < range.end);
        starting_before_end
            .checked_sub(1)
            .and_then(|last| self.reach.get(last))
            .is_some_and(|&furthest| furthest > range.start)
    }
}

pub(super) struct Numbering {
    pub followers: Vec<Edit>,
    pub shifts: Vec<u32>,
}

pub(super) fn plan(buf: &Buffer, insertions: &[Insertion], claims: &Claims) -> Numbering {
    let mut numbers: BTreeMap<usize, NumberEdit> = BTreeMap::new();
    for insertion in insertions {
        let first = insertion.caret_line + 1;
        let following = (first..buf.line_count()).map(|n| buf.line(n));
        for edit in continuation::renumber(&insertion.anchor, following, 1) {
            let line = first + edit.line;
            let Some(start) = buf.line_start(line) else {
                break;
            };
            let digits = start + edit.range.start..start + edit.range.end;
            if claims.overlaps(&digits) {
                break;
            }
            match numbers.entry(line) {
                Entry::Occupied(mut slot) => slot.get_mut().absorb(&edit),
                Entry::Vacant(slot) => {
                    slot.insert(edit);
                }
            }
        }
    }
    let mut earlier_on_line: BTreeMap<usize, u32> = BTreeMap::new();
    let shifts = insertions
        .iter()
        .map(|insertion| {
            let above = numbers
                .get(&insertion.anchor_line)
                .map_or(0, NumberEdit::shift);
            let siblings = earlier_on_line.entry(insertion.anchor_line).or_insert(0);
            let shift = above + *siblings;
            *siblings += 1;
            shift
        })
        .collect();
    let followers = numbers
        .into_iter()
        .filter_map(|(line, number)| {
            let start = buf.line_start(line)?;
            Some(Edit {
                start: start + number.range.start,
                end: start + number.range.end,
                insert: number.text,
            })
        })
        .collect();
    Numbering { followers, shifts }
}
