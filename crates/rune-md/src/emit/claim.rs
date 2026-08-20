use std::collections::BTreeMap;

use crate::icons::IconSet;
use rune_syntax::element::LineLocal;
use rune_syntax::syntax::TableRowInfo;
use rune_syntax::{LineDecor, SyntaxSpan, merge_overlapping};

pub(crate) type Accounted = Vec<Vec<(usize, usize)>>;

pub(crate) struct Sinks<'a> {
    pub(crate) spans: &'a mut [Vec<SyntaxSpan>],
    pub(crate) hidden: &'a mut Accounted,
    pub(crate) accounted: &'a mut Accounted,
}

pub(crate) struct EmitOut<'a> {
    spans: &'a mut [Vec<SyntaxSpan>],
    hidden: &'a mut Accounted,
    accounted: &'a mut Accounted,
    merged: Vec<BTreeMap<usize, usize>>,
    pub tables: &'a mut [Option<TableRowInfo>],
    pub width: u16,
    pub icons: &'a IconSet,
    pub decors: &'a mut [Option<LineDecor>],
}

pub(crate) struct Granted<'out, 'a> {
    out: &'out mut EmitOut<'a>,
    line: usize,
    pieces: Vec<(usize, usize)>,
}

impl Granted<'_, '_> {
    pub(crate) fn pieces(&self) -> &[(usize, usize)] {
        &self.pieces
    }

    pub(crate) fn push_visible(self, spans: Vec<SyntaxSpan>) {
        let within_grant = spans.iter().all(|span| {
            let range = span.range();
            self.pieces
                .iter()
                .any(|&(s, e)| s <= range.start && range.end <= e)
        });
        rune_core::assert_invariant!(within_grant, || {
            format!(
                "line {}: pushed span(s) fall outside the granted claim — producer bug (accounted would desync from what is drawn)",
                self.line
            )
        });
        let fully_covered = self
            .pieces
            .iter()
            .all(|&piece| piece_is_covered(&spans, piece));
        rune_core::assert_invariant!(fully_covered, || {
            format!(
                "line {}: granted piece(s) not fully covered by pushed span(s) — producer bug (accounted would record bytes nothing paints)",
                self.line
            )
        });
        if let Some(bucket) = self.out.spans.get_mut(self.line) {
            bucket.extend(spans);
        }
        if let Some(bucket) = self.out.accounted.get_mut(self.line) {
            bucket.extend(self.pieces.iter().copied());
        }
        self.out.mark_merged(self.line, &self.pieces);
    }

    pub(crate) fn record_hidden(self) {
        if let Some(bucket) = self.out.hidden.get_mut(self.line) {
            bucket.extend(self.pieces.iter().copied());
        }
        if let Some(bucket) = self.out.accounted.get_mut(self.line) {
            bucket.extend(self.pieces.iter().copied());
        }
        self.out.mark_merged(self.line, &self.pieces);
    }
}

fn piece_is_covered(spans: &[SyntaxSpan], piece: (usize, usize)) -> bool {
    let (piece_start, piece_end) = piece;
    if piece_start >= piece_end {
        return true;
    }
    let ranges: Vec<(usize, usize)> = spans
        .iter()
        .map(|span| {
            let r = span.range();
            (r.start, r.end)
        })
        .collect();
    merge_overlapping(ranges)
        .iter()
        .any(|&(s, e)| s <= piece_start && piece_end <= e)
}

pub(crate) struct Refused;

impl<'a> EmitOut<'a> {
    pub(crate) fn new(
        sinks: Sinks<'a>,
        tables: &'a mut [Option<TableRowInfo>],
        width: u16,
        icons: &'a IconSet,
        decors: &'a mut [Option<LineDecor>],
    ) -> Self {
        let merged = sinks
            .accounted
            .iter()
            .map(|line| {
                let mut map = BTreeMap::new();
                for &(s, e) in line {
                    insert_merged(&mut map, s, e);
                }
                map
            })
            .collect();
        Self {
            spans: sinks.spans,
            hidden: sinks.hidden,
            accounted: sinks.accounted,
            merged,
            tables,
            width,
            icons,
            decors,
        }
    }

    fn unclaimed(&self, line: usize, start: usize, end: usize) -> Vec<(usize, usize)> {
        match self.merged.get(line) {
            Some(existing) => unclaimed_subranges_in_merged(start, end, existing),
            None => {
                if end > start {
                    vec![(start, end)]
                } else {
                    Vec::new()
                }
            }
        }
    }

    fn mark_merged(&mut self, line: usize, pieces: &[(usize, usize)]) {
        let Some(map) = self.merged.get_mut(line) else {
            return;
        };
        for &(start, end) in pieces {
            insert_merged(map, start, end);
        }
    }

    pub(crate) fn claim_free(&mut self, ll: &LineLocal) -> Granted<'_, 'a> {
        let line = ll.line();
        let (start, end) = (ll.start(), ll.end());
        let pieces = self.unclaimed(line, start, end);

        let requested_len = end.saturating_sub(start);
        let kept_len: usize = pieces.iter().map(|&(s, e)| e - s).sum();
        rune_core::assert_invariant!(kept_len == requested_len, || {
            format!(
                "line {line}: visible claim [{start},{end}) overlaps {} already-claimed byte(s) — producer bug (content invented on the visible side)",
                requested_len - kept_len
            )
        });

        Granted {
            out: self,
            line,
            pieces,
        }
    }

    pub(crate) fn claim_whole(&mut self, ll: &LineLocal) -> Result<Granted<'_, 'a>, Refused> {
        let line = ll.line();
        if ll.is_empty() {
            return Ok(Granted {
                out: self,
                line,
                pieces: Vec::new(),
            });
        }
        let (start, end) = (ll.start(), ll.end());
        let pieces = self.unclaimed(line, start, end);
        if pieces == [(start, end)] {
            Ok(Granted {
                out: self,
                line,
                pieces,
            })
        } else {
            rune_core::assert_invariant!(false, || {
                format!(
                    "line {line}: whole claim [{start},{end}) is not entirely free — producer bug (overlaps a table row or task checkbox another producer already claimed)"
                )
            });
            Err(Refused)
        }
    }
}

fn unclaimed_subranges_in_merged(
    start: usize,
    end: usize,
    existing: &BTreeMap<usize, usize>,
) -> Vec<(usize, usize)> {
    if end <= start {
        return Vec::new();
    }
    let lower = existing
        .range(..=start)
        .next_back()
        .map_or(start, |(&s, _)| s);

    let mut result = Vec::new();
    let mut cursor = start;
    for (&s, &e) in existing.range(lower..end) {
        if e <= start {
            continue;
        }
        let clipped_start = s.max(start);
        let clipped_end = e.min(end);
        if clipped_start > cursor {
            result.push((cursor, clipped_start));
        }
        cursor = cursor.max(clipped_end);
    }
    if cursor < end {
        result.push((cursor, end));
    }
    result
}

fn insert_merged(map: &mut BTreeMap<usize, usize>, start: usize, end: usize) {
    if end <= start {
        return;
    }
    let (mut start, mut end) = (start, end);
    if let Some((&prev_start, &prev_end)) = map.range(..=start).next_back()
        && prev_end >= start
    {
        map.remove(&prev_start);
        start = prev_start;
        end = end.max(prev_end);
    }
    while let Some((&next_start, &next_end)) = map.range(start..).next() {
        if next_start > end {
            break;
        }
        map.remove(&next_start);
        end = end.max(next_end);
    }
    map.insert(start, end);
}

#[cfg(test)]
#[path = "claim_tests.rs"]
mod tests;
