//! Emitter (plan Context, "Emit -> wrap -> snapshot"): walks the `Block`/
//! `Inline` tree in model-line order (`walk::emit_block`), producing one
//! `SyntaxLine` per buffer line plus a `SyntaxSnapshot` for buffer<->syntax
//! coordinate conversion — a structural port of
//! `pkg/editor/display/{markdown_block,syntax_map,syntax_snapshot,
//! cellmap}.go`.
//!
//! Concealment is physical here, uniformly for block markers AND inline
//! delimiters: a `Rendered` element's marker/delimiter bytes are dropped
//! from the emitted text (recorded as a hidden range for coordinate
//! conversion) rather than kept-but-restyled. This is a deliberate
//! simplification of Go's model, where block-level markers (heading `"## "`,
//! blockquote `"> "`) stay in the emitted text and are hidden only by the
//! renderer, while inline delimiters are physically dropped — two policies
//! for one concept. Phase 1 unifies them: `Rendered` always means "the
//! markup bytes are not part of the syntax-space text", block or inline
//! alike, consistent with the plan's single `RevealState` used everywhere.
//!
//! Nested styling (bold-inside-italic) falls out of the tree via `StyleCtx`
//! (`style.rs`), an accumulator that lives only for the duration of the
//! walk — no `InlineMarks` bitfield is stored on any `SyntaxSpan` (plan
//! Context: "Nested styling ... falls out of the tree via the Emitter's
//! style stack — no `InlineMarks` bitfield").
//!
//! Every producer-bug invariant this module checks (an overlapping hidden
//! range in `syntax::build_line_conversions`, a duplicate visible claim in
//! `push_span_split_by_line`) is gated on [`STRICT_INVARIANTS`], never on
//! `cfg(debug_assertions)`: CONSTITUTION §1.3 requires an ORDINARY shipped
//! build — including an unoptimized debug one a developer might run
//! directly — to degrade gracefully on a producer bug, never panic on a
//! real user's document. Only a test run (or a build that explicitly opts
//! in via the `strict-invariants` feature) is allowed to treat the
//! violation as fatal. Graceful degradation itself (merge overlapping
//! hidden ranges; skip an already-claimed visible byte) runs in EVERY
//! build unconditionally — `STRICT_INVARIANTS` only gates whether a
//! detected violation additionally panics.

mod style;
mod syntax;
mod walk;

pub use style::StyleId;
pub use syntax::{CellMap, SyntaxLine, SyntaxSnapshot, SyntaxSpan};

use crate::element::block::Block;
use crate::element::{ByteRange, RevealState};
use crate::parse::{line_at, line_end_at, line_starts};
use syntax::build_line_conversions;

/// See the module docs: `true` only in test builds or when the
/// `strict-invariants` feature is explicitly enabled. `cfg!()` folds this
/// to a compile-time literal, so an `if STRICT_INVARIANTS { assert!(...) }`
/// guard compiles away entirely (dead code, zero cost) in an ordinary
/// shipped build.
pub(crate) const STRICT_INVARIANTS: bool = cfg!(any(test, feature = "strict-invariants"));

/// Every byte of every line is accounted for exactly once: either as part
/// of a VISIBLE span (pushed by `push_span_split_by_line`) or as a hidden
/// delimiter range (`hide_range`). `accounted[line]` is the union of both,
/// recorded so `fill_gaps` can find and surface whatever neither one
/// covered — trailing/leading whitespace, tabs, a bare `\r` before `\n`,
/// anything a comrak node's sourcepos doesn't happen to span — as ordinary
/// visible text rather than silently dropping it (a dropped byte is a data
/// hazard: the caret could no longer reach it, CONSTITUTION §0/§1.3).
pub(crate) type Accounted = Vec<Vec<(usize, usize)>>;

/// The chokepoint every range->line-bucket routine in this crate is built
/// on: splits `range` across every source line it touches and calls `f`
/// once per non-empty clipped `[seg_start, seg_end)` slice, already clamped
/// to that line's own bounds. A single range is NEVER assumed to stay
/// within one line — comrak can (and does) hand back a block's sourcepos
/// extending past its own visible content into a trailing blank/
/// whitespace-only line it absorbed (observed for `ThematicBreak`: `"# h\n
/// ---\n   "` reports the Hr's range running all the way to end-of-buffer,
/// past its own `"---"` line, into the trailing `"   "` line). Registering
/// that whole unclipped range under a single line bucket would silently
/// swallow the next line's bytes into this line's hidden-byte count —
/// exactly the shape `push_span_split_by_line`'s per-line loop already
/// guarded against, now shared so `hide_range`/`account` get the same
/// guarantee instead of their own (previously unsafe) single-line
/// shortcut.
fn for_each_line_slice(
    content: &str,
    starts: &[usize],
    range: ByteRange,
    mut f: impl FnMut(usize, usize, usize),
) {
    if range.is_empty() {
        return;
    }
    let first_line = line_at(starts, range.start);
    let last_line = line_at(starts, range.end.saturating_sub(1).max(range.start));
    for line in first_line..=last_line {
        let line_start = starts.get(line).copied().unwrap_or(0);
        let line_end = line_end_at(content.len(), starts, line);
        let seg_start = range.start.max(line_start);
        let seg_end = range.end.min(line_end);
        if seg_end > seg_start {
            f(line, seg_start, seg_end);
        }
    }
}

fn account(accounted: &mut Accounted, content: &str, starts: &[usize], range: ByteRange) {
    for_each_line_slice(content, starts, range, |line, s, e| {
        if let Some(bucket) = accounted.get_mut(line) {
            bucket.push((s, e));
        }
    });
}

/// The sub-ranges of `[start, end)` NOT already covered by `existing` (a
/// possibly unsorted, possibly-overlapping already-claimed set on the same
/// line) — the visible-side counterpart of `syntax::merge_overlapping`'s
/// hidden-side collapse. Reuses that same merge so both sides agree on
/// what "already claimed" means.
fn unclaimed_subranges(
    start: usize,
    end: usize,
    existing: &[(usize, usize)],
) -> Vec<(usize, usize)> {
    if end <= start {
        return Vec::new();
    }
    let mut sorted: Vec<(usize, usize)> =
        existing.iter().copied().filter(|&(s, e)| e > s).collect();
    sorted.sort_by_key(|&(s, _)| s);
    let merged = syntax::merge_overlapping(sorted);

    let mut result = Vec::new();
    let mut cursor = start;
    for (s, e) in merged {
        if e <= start || s >= end {
            continue; // doesn't intersect [start, end) at all
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

/// Port of `pkg/editor/display/cellmap.go:buildInlineCellMap`: one entry per
/// visual char, the absolute buffer offset it maps back to.
fn build_cell_map(content_start: usize, text: &str) -> CellMap {
    let mut cm = Vec::with_capacity(text.chars().count());
    let mut i = 0usize;
    for ch in text.chars() {
        cm.push((content_start + i) as i64);
        i += ch.len_utf8();
    }
    cm
}

/// The workhorse: split an absolute buffer range across the source lines it
/// covers and push one `SyntaxSpan` per line-slice. Builds a `cell_map` only
/// for `Rendered` spans (their text is always a direct, contiguous slice of
/// the buffer at this call site — concealed content minus its delimiters).
/// Every emitted slice is also recorded into `accounted` (see its docs).
///
/// HARDENING: before pushing, clips each line-slice against whatever
/// `accounted[line]` already claims (from an earlier visible span OR a
/// hidden range) via `unclaimed_subranges`, so a byte already emitted (or
/// hidden) is never emitted a second time — the visible-side counterpart
/// of `syntax::build_line_conversions`'s hidden-side merge. A hidden range
/// can be merged AFTER the fact because `build_line_conversions` runs once
/// over the whole set; a visible span becomes a real `SyntaxSpan` the
/// instant it's pushed, so this has to happen HERE, at the point of claim
/// (the class of bug this guards: an empty list item's marker running
/// onto its continuation line and re-showing bytes a nested blockquote's
/// own marker scan already claimed — content invented on the visible
/// side, content_range's mirror image of dropping a byte, both are a
/// §1.4.5 violation).
pub(crate) fn push_span_split_by_line(
    content: &str,
    starts: &[usize],
    range: ByteRange,
    style: StyleId,
    state: RevealState,
    out: &mut [Vec<SyntaxSpan>],
    accounted: &mut Accounted,
) {
    for_each_line_slice(content, starts, range, |line, seg_start, seg_end| {
        let existing = accounted.get(line).cloned().unwrap_or_default();
        let pieces = unclaimed_subranges(seg_start, seg_end, &existing);

        let requested_len = seg_end - seg_start;
        let kept_len: usize = pieces.iter().map(|&(s, e)| e - s).sum();
        if STRICT_INVARIANTS {
            assert!(
                kept_len == requested_len,
                "line {line}: visible claim [{seg_start},{seg_end}) overlaps {} already-claimed byte(s) — producer bug (content invented on the visible side)",
                requested_len - kept_len
            );
        }

        for (s, e) in pieces {
            let Some(text) = content.get(s..e) else {
                continue;
            };
            let cell_map = (state == RevealState::Rendered).then(|| build_cell_map(s, text));
            if let Some(bucket) = out.get_mut(line) {
                bucket.push(SyntaxSpan {
                    text: text.to_string(),
                    style,
                    state,
                    buffer_start: s,
                    buffer_end: e,
                    cell_map,
                });
            }
            if let Some(bucket) = accounted.get_mut(line) {
                bucket.push((s, e));
            }
        }
    });
}

/// Records an absolute buffer range as hidden (delimiter bytes dropped from
/// the emitted text) AND accounted for, in one call — the chokepoint every
/// concealed marker/delimiter in `walk.rs` routes through, so a hidden
/// range can never be pushed without also being accounted for (that
/// mismatch was BLOCKER 1: a per-LINE `touched` bool couldn't tell a
/// partially-covered line from a fully-covered one). Splits per line via
/// `for_each_line_slice` exactly like `push_span_split_by_line` — a
/// "delimiter" is not guaranteed single-line just because Phase-1 tokens
/// usually are (see `for_each_line_slice`'s docs for the counterexample
/// that proved this).
pub(crate) fn hide_range(
    hidden: &mut Accounted,
    accounted: &mut Accounted,
    content: &str,
    starts: &[usize],
    range: ByteRange,
) {
    for_each_line_slice(content, starts, range, |line, s, e| {
        if let Some(bucket) = hidden.get_mut(line) {
            bucket.push((s, e));
        }
    });
    account(accounted, content, starts, range);
}

/// The per-byte safety net (fixes BLOCKER 1): whatever no element's own
/// range covered — trailing/leading whitespace, tabs, a bare `\r` before
/// `\n`, indentation, anything a comrak sourcepos doesn't happen to span —
/// is surfaced as ordinary visible text rather than silently dropped.
/// Merges each line's `accounted` ranges (both visible spans AND hidden
/// delimiters — see `Accounted`'s docs), finds the complement within the
/// line's full byte range, and inserts a Revealed span per gap in the
/// correct buffer-order position (the final per-line sort by
/// `buffer_start`).
fn fill_gaps(content: &str, starts: &[usize], accounted: &Accounted, out: &mut [Vec<SyntaxSpan>]) {
    for line in 0..starts.len() {
        let line_start = starts.get(line).copied().unwrap_or(0);
        let line_end = line_end_at(content.len(), starts, line).max(line_start);

        let mut ranges: Vec<(usize, usize)> = accounted.get(line).cloned().unwrap_or_default();
        ranges.sort_by_key(|&(s, _)| s);

        let mut cursor = line_start;
        let mut gaps: Vec<(usize, usize)> = Vec::new();
        for (s, e) in ranges {
            let s = s.clamp(line_start, line_end);
            let e = e.clamp(line_start, line_end);
            if s > cursor {
                gaps.push((cursor, s));
            }
            if e > cursor {
                cursor = e;
            }
        }
        if cursor < line_end {
            gaps.push((cursor, line_end));
        }
        if gaps.is_empty() {
            continue;
        }

        let Some(bucket) = out.get_mut(line) else {
            continue;
        };
        for (s, e) in gaps {
            if e <= s {
                continue;
            }
            let Some(text) = content.get(s..e) else {
                continue;
            };
            bucket.push(SyntaxSpan {
                text: text.to_string(),
                style: StyleId::Text,
                state: RevealState::Revealed,
                buffer_start: s,
                buffer_end: e,
                cell_map: None,
            });
        }
        // Gap-fill spans are appended out of buffer order relative to
        // whatever spans already sit in `bucket` — restore document order
        // so the line's spans concatenate back to the correct text.
        bucket.sort_by_key(|s| s.buffer_start);
    }
}

/// The crate's one Emit entry point: `Block` tree -> per-line `SyntaxLine`s
/// and a `SyntaxSnapshot` for coordinate conversion. `DocMachine::snapshot`
/// is the only caller.
pub fn emit(content: &str, blocks: &[Block]) -> (Vec<SyntaxLine>, SyntaxSnapshot) {
    let starts = line_starts(content);
    let mut spans: Vec<Vec<SyntaxSpan>> = vec![Vec::new(); starts.len()];
    let mut hidden: Accounted = vec![Vec::new(); starts.len()];
    let mut accounted: Accounted = vec![Vec::new(); starts.len()];

    for b in blocks {
        walk::emit_block(content, &starts, b, &mut spans, &mut hidden, &mut accounted);
    }
    fill_gaps(content, &starts, &accounted, &mut spans);

    let lines: Vec<SyntaxLine> = spans
        .into_iter()
        .map(|spans| SyntaxLine { spans })
        .collect();
    let line_convs = build_line_conversions(&starts, &hidden);
    (lines, SyntaxSnapshot { line_convs })
}

#[cfg(test)]
mod tests;
