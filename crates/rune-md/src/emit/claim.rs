//! The claim primitive itself, split out from `emit::mod` (WP5) as a
//! SIBLING of `walk`/`walk_inline`/`table`/`decor` rather than staying their
//! parent: Rust privacy makes a private field reachable from the defining
//! module and every descendant, so as long as `EmitOut` lived in `emit::mod`
//! its private `spans`/`hidden`/`accounted` stayed reachable from those four
//! child modules regardless of what the public methods below claimed to
//! enforce. Defining `EmitOut` here instead makes that reach a compile
//! error: `walk`/`walk_inline`/`table`/`decor` are this module's siblings,
//! not its descendants, so only `EmitOut`'s own methods can touch its three
//! sinks directly.
//!
//! `EmitOut::new` exists because `emit::mod` (the struct's parent, not a
//! descendant either) needs to build one; every other field stays `pub`
//! since `tables`/`width`/`icons`/`decors` were never part of the claim
//! contract.

use crate::icons::IconSet;
use rune_syntax::syntax::TableRowInfo;
use rune_syntax::{LineDecor, SyntaxSpan, merge_overlapping};

/// Every byte of every line is accounted for exactly once: either as part
/// of a VISIBLE span (pushed by `push_visible`) or as a hidden delimiter
/// range (`record_hidden`). `accounted[line]` is the union of both,
/// recorded so `fill_gaps` can find and surface whatever neither one
/// covered — trailing/leading whitespace, tabs, a bare `\r` before `\n`,
/// anything a comrak node's sourcepos doesn't happen to span — as ordinary
/// visible text rather than silently dropping it (a dropped byte is a data
/// hazard: the caret could no longer reach it).
pub(crate) type Accounted = Vec<Vec<(usize, usize)>>;

/// The out-params every `emit_block`/`emit_inline` call threads (`emit::mod`
/// docs explain why they are bundled rather than passed loose). `spans`,
/// `hidden` and `accounted` are the three sinks a claim guards; the rest
/// were never part of that contract, so they stay `pub`.
pub(crate) struct EmitOut<'a> {
    spans: &'a mut [Vec<SyntaxSpan>],
    hidden: &'a mut Accounted,
    accounted: &'a mut Accounted,
    pub tables: &'a mut [Option<TableRowInfo>],
    pub width: u16,
    pub icons: &'a IconSet,
    pub decors: &'a mut [Option<LineDecor>],
}

/// A claim on `line`'s unclaimed sub-ranges of a requested byte range,
/// returned by `EmitOut::claim_free`/`claim_whole` and spent by
/// `push_visible` or `record_hidden` — spending is what records the claim
/// into `accounted`, so a `Granted` dropped without being spent leaves no
/// trace and its bytes still reach `fill_gaps`. Neither `Copy` nor `Clone`,
/// and its fields are private to this module, so a producer cannot
/// fabricate one and cannot hold two at once for the same call.
pub(crate) struct Granted {
    line: usize,
    pieces: Vec<(usize, usize)>,
}

impl Granted {
    /// The sub-ranges this claim actually covers — read-only, so a caller
    /// can build the visible spans it is about to hand to `push_visible`
    /// without being able to spend the claim twice or fabricate a second
    /// one.
    pub(crate) fn pieces(&self) -> &[(usize, usize)] {
        &self.pieces
    }
}

/// `claim_whole`'s error case: the requested range was not entirely free,
/// so no `Granted` was produced. Carries nothing — the caller's only
/// legitimate move is to skip its substitution, never to guess at a partial
/// one (`claim_whole`'s own docs).
pub(crate) struct Refused;

impl<'a> EmitOut<'a> {
    pub(crate) fn new(
        spans: &'a mut [Vec<SyntaxSpan>],
        hidden: &'a mut Accounted,
        accounted: &'a mut Accounted,
        tables: &'a mut [Option<TableRowInfo>],
        width: u16,
        icons: &'a IconSet,
        decors: &'a mut [Option<LineDecor>],
    ) -> Self {
        Self {
            spans,
            hidden,
            accounted,
            tables,
            width,
            icons,
            decors,
        }
    }

    /// Grants whatever sub-ranges of `[start, end)` on `line` are not
    /// already in `accounted` (visible span or hidden range, either
    /// counts). `push_visible`/`record_hidden` are the only ways to spend
    /// what this returns. A producer whose own range arithmetic overlaps
    /// another producer's claim is a bug, not a legitimate outcome — unlike
    /// `claim_whole`, this asserts on that instead of returning a refusal.
    pub(crate) fn claim_free(&mut self, line: usize, start: usize, end: usize) -> Granted {
        let existing = self.accounted.get(line).cloned().unwrap_or_default();
        let pieces = unclaimed_subranges(start, end, &existing);

        let requested_len = end.saturating_sub(start);
        let kept_len: usize = pieces.iter().map(|&(s, e)| e - s).sum();
        rune_core::assert_invariant!(kept_len == requested_len, || {
            format!(
                "line {line}: visible claim [{start},{end}) overlaps {} already-claimed byte(s) — producer bug (content invented on the visible side)",
                requested_len - kept_len
            )
        });

        Granted { line, pieces }
    }

    /// Grants `[start, end)` on `line` whole, or refuses it outright —
    /// never a partial piece. A substituting producer (a table row, a task
    /// checkbox glyph) draws one replacement string for the whole range it
    /// claims; there is no way to draw part of that replacement into
    /// whatever sub-ranges happen to survive an overlap, so unlike
    /// `claim_free` a refusal here is an ordinary outcome, not a producer
    /// bug — it never asserts.
    pub(crate) fn claim_whole(
        &mut self,
        line: usize,
        start: usize,
        end: usize,
    ) -> Result<Granted, Refused> {
        let existing = self.accounted.get(line).cloned().unwrap_or_default();
        let pieces = unclaimed_subranges(start, end, &existing);
        if pieces == [(start, end)] {
            Ok(Granted { line, pieces })
        } else {
            Err(Refused)
        }
    }

    /// Spends `granted` by pushing `spans` as that line's visible content
    /// and recording its pieces into `accounted`.
    pub(crate) fn push_visible(&mut self, granted: Granted, spans: Vec<SyntaxSpan>) {
        if let Some(bucket) = self.spans.get_mut(granted.line) {
            bucket.extend(spans);
        }
        if let Some(bucket) = self.accounted.get_mut(granted.line) {
            bucket.extend(granted.pieces.iter().copied());
        }
    }

    /// Spends `granted` by recording its pieces as hidden (delimiter bytes
    /// dropped from the emitted text) and into `accounted`.
    pub(crate) fn record_hidden(&mut self, granted: Granted) {
        if let Some(bucket) = self.hidden.get_mut(granted.line) {
            bucket.extend(granted.pieces.iter().copied());
        }
        if let Some(bucket) = self.accounted.get_mut(granted.line) {
            bucket.extend(granted.pieces.iter().copied());
        }
    }
}

/// The sub-ranges of `[start, end)` NOT already covered by `existing` (a
/// possibly unsorted, possibly-overlapping already-claimed set on the same
/// line) — the visible-side counterpart of `rune_syntax`'s
/// `merge_overlapping`'s hidden-side collapse. Reuses that same merge so
/// both sides agree on what "already claimed" means.
fn unclaimed_subranges(
    start: usize,
    end: usize,
    existing: &[(usize, usize)],
) -> Vec<(usize, usize)> {
    if end <= start {
        return Vec::new();
    }
    let unsorted: Vec<(usize, usize)> = existing.iter().copied().filter(|&(s, e)| e > s).collect();
    let merged = merge_overlapping(unsorted);

    let mut result = Vec::new();
    let mut cursor = start;
    for (s, e) in merged {
        if e <= start || s >= end {
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

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]
    use super::*;

    /// The visible-side dedup computation, tested in isolation (no assert
    /// involved — `unclaimed_subranges` itself never panics, it just
    /// computes what's left). Mirrors "- \n  > q"'s shape: a claim
    /// ([0,8)) that overlaps a bit already claimed in the middle ([2,6)),
    /// leaving two disjoint unclaimed pieces.
    #[test]
    fn unclaimed_subranges_skips_already_claimed_bytes() {
        let pieces = unclaimed_subranges(0, 8, &[(2, 6)]);
        assert_eq!(pieces, vec![(0, 2), (6, 8)]);

        assert_eq!(
            unclaimed_subranges(2, 6, &[(0, 8)]),
            Vec::<(usize, usize)>::new()
        );

        assert_eq!(unclaimed_subranges(0, 4, &[(10, 12)]), vec![(0, 4)]);

        assert_eq!(
            unclaimed_subranges(0, 10, &[(6, 8), (1, 3), (3, 4)]),
            vec![(0, 1), (4, 6), (8, 10)]
        );
    }

    /// A `Granted` dropped without being spent through `push_visible` or
    /// `record_hidden` leaves `accounted` untouched — the bytes it would
    /// have claimed still reach `fill_gaps` instead of vanishing.
    #[test]
    fn dropped_claim_leaves_accounted_unchanged() {
        let mut spans: Vec<Vec<SyntaxSpan>> = vec![Vec::new()];
        let mut hidden: Accounted = vec![Vec::new()];
        let mut accounted: Accounted = vec![Vec::new()];
        let mut tables: Vec<Option<TableRowInfo>> = vec![None];
        let mut decors: Vec<Option<LineDecor>> = vec![None];
        let icons = IconSet::unicode();
        let mut out = EmitOut::new(
            &mut spans,
            &mut hidden,
            &mut accounted,
            &mut tables,
            80,
            &icons,
            &mut decors,
        );

        let granted = out.claim_free(0, 0, 4);
        drop(granted);

        assert_eq!(accounted[0], Vec::<(usize, usize)>::new());
    }

    /// A `claim_whole` refusal — the requested range partially overlaps an
    /// already-accounted piece — produces no `Granted` (the `Result`'s
    /// `Err` arm carries none by construction) and leaves `accounted`
    /// exactly as it was, so the bytes still reach `fill_gaps`.
    #[test]
    fn refused_whole_claim_yields_no_granted_and_leaves_accounted_untouched() {
        let mut spans: Vec<Vec<SyntaxSpan>> = vec![Vec::new()];
        let mut hidden: Accounted = vec![Vec::new()];
        let mut accounted: Accounted = vec![vec![(2, 4)]];
        let mut tables: Vec<Option<TableRowInfo>> = vec![None];
        let mut decors: Vec<Option<LineDecor>> = vec![None];
        let icons = IconSet::unicode();
        let mut out = EmitOut::new(
            &mut spans,
            &mut hidden,
            &mut accounted,
            &mut tables,
            80,
            &icons,
            &mut decors,
        );

        let result = out.claim_whole(0, 0, 8);

        assert!(result.is_err());
        assert_eq!(accounted[0], vec![(2, 4)]);
    }
}
