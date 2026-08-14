//! The claim primitive itself, split out from `emit::mod` as a SIBLING of
//! `walk`/`walk_inline`/`table`/`decor` rather than staying their parent:
//! Rust privacy makes a private field reachable from the defining module
//! and every descendant, so as long as `EmitOut` lived in `emit::mod` its
//! private `spans`/`hidden`/`accounted` stayed reachable from those four
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
use rune_syntax::element::LineLocal;
use rune_syntax::syntax::TableRowInfo;
use rune_syntax::{LineDecor, SyntaxSpan, merge_overlapping};

/// Every byte of every line is accounted for exactly once: either as part
/// of a VISIBLE span (pushed by `Granted::push_visible`) or as a hidden
/// delimiter range (`Granted::record_hidden`). `accounted[line]` is the
/// union of both, recorded so `fill_gaps` can find and surface whatever
/// neither one covered — trailing/leading whitespace, tabs, a bare `\r`
/// before `\n`, anything a comrak node's sourcepos doesn't happen to span
/// — as ordinary visible text rather than silently dropping it (a dropped
/// byte is a data hazard: the caret could no longer reach it).
pub(crate) type Accounted = Vec<Vec<(usize, usize)>>;

/// The three sinks a claim guards, named so a caller building `EmitOut`
/// binds each by name (field-init shorthand) instead of by position —
/// `hidden` and `accounted` are adjacent and identically typed, so a
/// positional constructor let them transpose silently.
pub(crate) struct Sinks<'a> {
    pub(crate) spans: &'a mut [Vec<SyntaxSpan>],
    pub(crate) hidden: &'a mut Accounted,
    pub(crate) accounted: &'a mut Accounted,
}

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
/// trace and its bytes still reach `fill_gaps`. It borrows the `EmitOut`
/// that granted it for as long as it stays unspent, so the borrow checker
/// refuses a second claim on the same `EmitOut` before this one is spent:
/// `push_visible`/`record_hidden` consume `self`, releasing that borrow
/// only once the claim has actually been recorded.
pub(crate) struct Granted<'out, 'a> {
    out: &'out mut EmitOut<'a>,
    line: usize,
    pieces: Vec<(usize, usize)>,
}

impl Granted<'_, '_> {
    /// The sub-ranges this claim actually covers — read-only, so a caller
    /// can build the visible spans it is about to hand to `push_visible`
    /// without being able to spend the claim twice or fabricate a second
    /// one.
    pub(crate) fn pieces(&self) -> &[(usize, usize)] {
        &self.pieces
    }

    /// Spends this claim by pushing `spans` as `line`'s visible content and
    /// recording its pieces into `accounted`. Every pushed span's range
    /// must fall inside one of the granted pieces — a producer that claims
    /// range A and pushes spans for range B would desync `accounted` from
    /// what is actually drawn, silently reopening the dropped-byte hazard
    /// `accounted` exists to close.
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
    }

    /// Spends this claim by recording its pieces as hidden (delimiter bytes
    /// dropped from the emitted text) and into `accounted`.
    pub(crate) fn record_hidden(self) {
        if let Some(bucket) = self.out.hidden.get_mut(self.line) {
            bucket.extend(self.pieces.iter().copied());
        }
        if let Some(bucket) = self.out.accounted.get_mut(self.line) {
            bucket.extend(self.pieces.iter().copied());
        }
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

/// `claim_whole`'s error case: the requested range was not entirely free,
/// so no `Granted` was produced. Carries nothing — the caller's only
/// legitimate move is to skip its substitution, never to guess at a partial
/// one (`claim_whole`'s own docs).
pub(crate) struct Refused;

impl<'a> EmitOut<'a> {
    pub(crate) fn new(
        sinks: Sinks<'a>,
        tables: &'a mut [Option<TableRowInfo>],
        width: u16,
        icons: &'a IconSet,
        decors: &'a mut [Option<LineDecor>],
    ) -> Self {
        Self {
            spans: sinks.spans,
            hidden: sinks.hidden,
            accounted: sinks.accounted,
            tables,
            width,
            icons,
            decors,
        }
    }

    /// The sub-ranges of `[start, end)` on `line` not already in
    /// `accounted` — the lookup both `claim_free` and `claim_whole` need
    /// before applying their own (different) refusal policy.
    fn unclaimed(&self, line: usize, start: usize, end: usize) -> Vec<(usize, usize)> {
        let existing = self.accounted.get(line).cloned().unwrap_or_default();
        unclaimed_subranges(start, end, &existing)
    }

    /// Grants whatever sub-ranges of `[start, end)` on `line` are not
    /// already in `accounted` (visible span or hidden range, either
    /// counts). `push_visible`/`record_hidden` are the only ways to spend
    /// what this returns. A producer whose own range arithmetic overlaps
    /// another producer's claim is a bug, not a legitimate outcome — unlike
    /// `claim_whole`, this asserts on that instead of returning a refusal.
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

    /// Grants `[start, end)` on `line` whole, or refuses it outright —
    /// never a partial piece. A substituting producer (a table row, a task
    /// checkbox glyph) draws one replacement string for the whole range it
    /// claims; there is no way to draw part of that replacement into
    /// whatever sub-ranges happen to survive an overlap, so a refusal here
    /// is an ordinary outcome for the CALLER, which is expected to skip its
    /// substitution rather than treat it as fatal. An empty range conflicts
    /// with nothing and is always granted. A non-empty range that is not
    /// entirely free is still a PRODUCER bug — the caller degrades
    /// gracefully, but the mismatch is asserted exactly like `claim_free`'s.
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
    #![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)]
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
            Sinks {
                spans: &mut spans,
                hidden: &mut hidden,
                accounted: &mut accounted,
            },
            &mut tables,
            80,
            &icons,
            &mut decors,
        );

        let ll = LineLocal::clip(0, 0..4, 0..4).unwrap();
        let granted = out.claim_free(&ll);
        drop(granted);

        assert_eq!(accounted[0], Vec::<(usize, usize)>::new());
    }

    /// An empty range conflicts with nothing, so `claim_whole` grants it —
    /// never refuses it — regardless of what else the line already has
    /// accounted for.
    #[test]
    fn claim_whole_grants_an_empty_range_even_when_the_line_is_fully_claimed() {
        let mut spans: Vec<Vec<SyntaxSpan>> = vec![Vec::new()];
        let mut hidden: Accounted = vec![Vec::new()];
        let mut accounted: Accounted = vec![vec![(0, 8)]];
        let mut tables: Vec<Option<TableRowInfo>> = vec![None];
        let mut decors: Vec<Option<LineDecor>> = vec![None];
        let icons = IconSet::unicode();
        let mut out = EmitOut::new(
            Sinks {
                spans: &mut spans,
                hidden: &mut hidden,
                accounted: &mut accounted,
            },
            &mut tables,
            80,
            &icons,
            &mut decors,
        );

        let ll = LineLocal::clip(0, 0..8, 4..4).unwrap();
        let result = out.claim_whole(&ll);

        assert!(result.is_ok());
    }

    /// A `claim_whole` refusal — the requested range partially overlaps an
    /// already-accounted piece — is a producer bug, so it asserts under
    /// `strict-invariants`/test builds instead of returning silently.
    #[test]
    #[should_panic(expected = "is not entirely free")]
    fn claim_whole_asserts_on_a_refused_overlap() {
        let mut spans: Vec<Vec<SyntaxSpan>> = vec![Vec::new()];
        let mut hidden: Accounted = vec![Vec::new()];
        let mut accounted: Accounted = vec![vec![(2, 4)]];
        let mut tables: Vec<Option<TableRowInfo>> = vec![None];
        let mut decors: Vec<Option<LineDecor>> = vec![None];
        let icons = IconSet::unicode();
        let mut out = EmitOut::new(
            Sinks {
                spans: &mut spans,
                hidden: &mut hidden,
                accounted: &mut accounted,
            },
            &mut tables,
            80,
            &icons,
            &mut decors,
        );

        let ll = LineLocal::clip(0, 0..8, 0..8).unwrap();
        let _ = out.claim_whole(&ll);
    }

    /// The producer-bug path above only proves the assert fires; this
    /// checks the state a refusal leaves behind — `accounted` untouched, so
    /// the bytes still reach `fill_gaps` — using `catch_unwind` to look past
    /// the panic this test binary's own armed assert raises.
    #[test]
    fn refused_whole_claim_leaves_accounted_untouched() {
        let mut spans: Vec<Vec<SyntaxSpan>> = vec![Vec::new()];
        let mut hidden: Accounted = vec![Vec::new()];
        let mut accounted: Accounted = vec![vec![(2, 4)]];
        let mut tables: Vec<Option<TableRowInfo>> = vec![None];
        let mut decors: Vec<Option<LineDecor>> = vec![None];
        let icons = IconSet::unicode();
        let mut out = EmitOut::new(
            Sinks {
                spans: &mut spans,
                hidden: &mut hidden,
                accounted: &mut accounted,
            },
            &mut tables,
            80,
            &icons,
            &mut decors,
        );

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let ll = LineLocal::clip(0, 0..8, 0..8).unwrap();
            out.claim_whole(&ll).is_err()
        }));

        assert!(result.is_err());
        let _ = out;
        assert_eq!(accounted[0], vec![(2, 4)]);
    }

    #[test]
    #[should_panic(expected = "not fully covered")]
    fn push_visible_catches_a_dropped_span_leaving_a_granted_piece_uncovered() {
        let mut spans: Vec<Vec<SyntaxSpan>> = vec![Vec::new()];
        let mut hidden: Accounted = vec![Vec::new()];
        let mut accounted: Accounted = vec![Vec::new()];
        let mut tables: Vec<Option<TableRowInfo>> = vec![None];
        let mut decors: Vec<Option<LineDecor>> = vec![None];
        let icons = IconSet::unicode();
        let mut out = EmitOut::new(
            Sinks {
                spans: &mut spans,
                hidden: &mut hidden,
                accounted: &mut accounted,
            },
            &mut tables,
            80,
            &icons,
            &mut decors,
        );

        let ll = LineLocal::clip(0, 0..4, 0..4).unwrap();
        let granted = out.claim_free(&ll);
        granted.push_visible(Vec::new());
    }
}
