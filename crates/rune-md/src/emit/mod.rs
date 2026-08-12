//! Emitter (plan Context, "Emit -> wrap -> snapshot"): walks the `Block`/
//! `Inline` tree in model-line order (`walk::emit_block`), producing one
//! `SyntaxLine` per buffer line plus a `SyntaxSnapshot` for buffer<->syntax
//! coordinate conversion.
//!
//! Concealment is physical here, uniformly for block markers AND inline
//! delimiters: a `Rendered` element's marker/delimiter bytes are dropped
//! from the emitted text (recorded as a hidden range for coordinate
//! conversion) rather than kept-but-restyled. This deliberately unifies
//! block and inline concealment under one policy, rather than block-level
//! markers (heading `"## "`, blockquote `"> "`) staying in the emitted text
//! hidden only by the renderer while inline delimiters are physically
//! dropped — two policies for one concept. `Rendered` always means "the
//! markup bytes are not part of the syntax-space text", block or inline
//! alike, consistent with the single `RevealState` used everywhere.
//!
//! Nested styling (bold-inside-italic) falls out of the tree via `StyleCtx`
//! (`style.rs`), an accumulator that lives only for the duration of the
//! walk — no `InlineMarks` bitfield is stored on any `SyntaxSpan` (plan
//! Context: "Nested styling ... falls out of the tree via the Emitter's
//! style stack — no `InlineMarks` bitfield").
//!
//! Every producer-bug invariant this module checks (a duplicate visible
//! claim in `push_span_split_by_line`) is gated on `assert_invariant`,
//! never on `cfg(debug_assertions)`: an ORDINARY shipped build — including
//! an unoptimized debug one a developer might run directly — must degrade
//! gracefully on a producer bug, never panic on a real user's document.
//! Only a test run (or a build that explicitly opts
//! in via this crate's own `strict-invariants` feature) is allowed to treat
//! the violation as fatal. Graceful degradation itself (skip an
//! already-claimed visible byte) runs in EVERY build unconditionally —
//! the strict-invariants gate only decides whether a detected violation
//! additionally panics. The sibling check over an overlapping HIDDEN range
//! moved to `rune-syntax`'s `syntax::build_line_conversions` (WP3), gated
//! by that crate's own `strict-invariants` feature — each crate's gate
//! governs only its own invariants.

// `pub(crate)`, not private: `crate::table::render`/`crate::table::layout`
// (siblings of `emit`, not descendants) resolve table cell/border text
// against the SAME scope table this module's own walk uses
// (`style::table_scope`/`table_header_scope`/`table_separator_scope`/
// `table_border_scope`/`text_scope`/`code_scope`/`link_scope`) — one
// canonical scope resolver, not a second one reimplemented in `table::`.
mod claim;
mod decor;
pub(crate) mod style;
mod table;
mod walk;
mod walk_inline;

use crate::element::block::Block;
use crate::icons::IconSet;
use crate::parse::{line_at, line_end_at, line_starts};
use rune_core::assert_invariant;
use rune_syntax::element::{ByteRange, RevealState};
use rune_syntax::syntax::TableRowInfo;
use rune_syntax::{LineDecor, ScopeId, SyntaxLine, SyntaxSnapshot, SyntaxSpan};

pub(crate) use claim::{Accounted, EmitOut, Sinks};

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

/// The workhorse: split an absolute buffer range across the source lines it
/// covers and push one `SyntaxSpan` per line-slice. Builds a `cell_map` only
/// for `Rendered` spans (their text is always a direct, contiguous slice of
/// the buffer at this call site — concealed content minus its delimiters).
/// Claims through `EmitOut::claim_free`, so a byte already emitted or hidden
/// is never emitted a second time (the class of bug this guards: an empty
/// list item's marker running onto its continuation line and re-showing
/// bytes a nested blockquote's own marker scan already claimed).
pub(crate) fn push_span_split_by_line(
    content: &str,
    starts: &[usize],
    range: ByteRange,
    scope: ScopeId,
    state: RevealState,
    out: &mut EmitOut,
) {
    for_each_line_slice(content, starts, range, |line, seg_start, seg_end| {
        let granted = out.claim_free(line, seg_start, seg_end);
        let spans: Vec<SyntaxSpan> = granted
            .pieces()
            .iter()
            .filter_map(|&(s, e)| build_line_span(content, line, s, e, scope, state))
            .collect();
        granted.push_visible(spans);
    });
}

/// A producer's range arithmetic (e.g. a delimiter derived by subtracting a
/// fixed byte count from a multibyte-char-adjacent position) can land inside
/// a char instead of on its boundary — `content.get` then returns `None`.
/// These are the user's own bytes: snapping the range outward to the
/// nearest valid boundaries and emitting verbatim is always safe, whereas
/// dropping the span would vanish real content from the display.
fn build_line_span(
    content: &str,
    line: usize,
    s: usize,
    e: usize,
    scope: ScopeId,
    state: RevealState,
) -> Option<SyntaxSpan> {
    if state != RevealState::Rendered {
        return Some(SyntaxSpan::identical(content, scope, s..e));
    }
    let (s, e, text) = match content.get(s..e) {
        Some(text) => (s, e, text),
        None => {
            let snapped_s = content.floor_char_boundary(s);
            let snapped_e = content.ceil_char_boundary(e);
            let snapped_ok = snapped_s == s && snapped_e == e;
            assert_invariant!(snapped_ok, || {
                format!(
                    "line {line}: span [{s},{e}) is not on a char boundary — producer bug; snapped outward to [{snapped_s},{snapped_e})"
                )
            });
            let text = content.get(snapped_s..snapped_e)?;
            (snapped_s, snapped_e, text)
        }
    };
    Some(SyntaxSpan::substituted(s, text.to_string(), scope, s..e))
}

/// Records an absolute buffer range as hidden (delimiter bytes dropped from
/// the emitted text) AND accounted for, in one call — the chokepoint every
/// concealed marker/delimiter in `walk.rs` routes through, so a hidden
/// range can never be recorded without also being accounted for. Splits per
/// line via `for_each_line_slice` exactly like `push_span_split_by_line` —
/// a "delimiter" is not guaranteed single-line just because Phase-1 tokens
/// usually are (see `for_each_line_slice`'s docs for the counterexample
/// that proved this). Claims through the SAME `EmitOut::claim_free` the
/// visible side uses, so a visible-then-hidden overlap (the walk order
/// `hide(open) -> emit children -> hide(close)` makes a later hidden claim
/// landing on an already-emitted visible byte reachable) is clipped and
/// asserted on instead of double-counted.
pub(crate) fn hide_range(content: &str, starts: &[usize], range: ByteRange, out: &mut EmitOut) {
    for_each_line_slice(content, starts, range, |line, s, e| {
        out.claim_free(line, s, e).record_hidden();
    });
}

/// The per-byte safety net (fixes BLOCKER 1): whatever no element's own
/// range covered — trailing/leading whitespace, tabs, a bare `\r` before
/// `\n`, indentation, anything a comrak sourcepos doesn't happen to span —
/// is surfaced as ordinary visible text rather than silently dropped.
/// Merges each line's `accounted` ranges (both visible spans AND hidden
/// delimiters — see `Accounted`'s docs), finds the complement within the
/// line's full byte range, and inserts an `Identical` span per gap in the
/// correct buffer-order position (the final per-line sort by `range`'s
/// start).
///
/// `base` is the scope those gap spans carry. It is a parameter rather
/// than a fixed `style::text_scope()` because a non-markdown document
/// parses to an empty block list, which makes THIS pass the sole producer
/// of its every span — so the document's kind can only reach the screen
/// through here (see `style::base_scope`).
fn fill_gaps(
    content: &str,
    starts: &[usize],
    accounted: &Accounted,
    base: ScopeId,
    out: &mut [Vec<SyntaxSpan>],
) {
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
            // No local char-boundary guard needed: `SyntaxSpan::identical`
            // clamps a bad range at construction instead of this call site
            // having to check and skip it first.
            bucket.push(SyntaxSpan::identical(content, base, s..e));
        }
        // Gap-fill spans are appended out of buffer order relative to
        // whatever spans already sit in `bucket` — restore document order
        // so the line's spans concatenate back to the correct text.
        bucket.sort_by_key(|s| s.range().start);
    }
}

/// The crate's one Emit entry point: `Block` tree -> per-line `SyntaxLine`s
/// and a `SyntaxSnapshot` for coordinate conversion. `DocMachine::snapshot`
/// is the only caller. `width` is `DocMachine`'s own `self.wrap.width` — a
/// PARAMETER, never a value this function or any element caches a copy of
/// (plan architectural decision 4: "a value has exactly one writer", and
/// `DocMachine`'s `WrapState` is that one writer). Carried unchanged into
/// `EmitOut::width` so the one arm that needs it — `Block::Table`'s
/// Grid/Wrapped/Pivoted layout selector (`table::layout::choose`) — reads it
/// without a second breaking change to this signature or every one of its
/// callers; every other producer ignores the field entirely.
pub fn emit(content: &str, blocks: &[Block], width: u16) -> (Vec<SyntaxLine>, SyntaxSnapshot) {
    emit_with(
        content,
        blocks,
        width,
        &IconSet::unicode(),
        style::text_scope(),
    )
}

/// `emit`'s full form (plan WP2.S4/B2 resolution): identical to `emit`
/// except decor producers draw their glyphs from `icons` instead of the
/// unicode-tier default, and unclaimed bytes fall back to `base` instead of
/// the plain-prose scope. `emit` is a thin wrapper over this so the ~90
/// existing 3-arg call sites across the workspace keep compiling unchanged;
/// a markdown document's gaps ARE prose, so the wrapper's fixed `text` is
/// the right answer for all of them.
pub fn emit_with(
    content: &str,
    blocks: &[Block],
    width: u16,
    icons: &IconSet,
    base: ScopeId,
) -> (Vec<SyntaxLine>, SyntaxSnapshot) {
    let starts = line_starts(content);
    let mut spans: Vec<Vec<SyntaxSpan>> = vec![Vec::new(); starts.len()];
    let mut hidden: Accounted = vec![Vec::new(); starts.len()];
    let mut accounted: Accounted = vec![Vec::new(); starts.len()];
    let mut tables: Vec<Option<TableRowInfo>> = (0..starts.len()).map(|_| None).collect();
    let mut decors: Vec<Option<LineDecor>> = (0..starts.len()).map(|_| None).collect();

    let mut out = EmitOut::new(
        Sinks {
            spans: &mut spans,
            hidden: &mut hidden,
            accounted: &mut accounted,
        },
        &mut tables,
        width,
        icons,
        &mut decors,
    );
    for b in blocks {
        walk::emit_block(content, &starts, b, 0, &mut out);
    }
    fill_gaps(content, &starts, &accounted, base, &mut spans);

    // MIXED-INDEX SEAM fallout (verification round 7): a producer's own
    // CALL ORDER is not the same thing as buffer-BYTE order, and this
    // crate never used to need the distinction — every producer used to
    // push a line's OWN spans strictly left-to-right (a container's
    // repeating marker always sat at the very START of its own buffer
    // line, so "walk markers, then walk children" happened to match byte
    // order by construction). Once a blockquote marker can legitimately
    // sit MID-buffer-line (a marker on a comrak line that follows a bare
    // `\r` earlier in the SAME buffer line — exactly what round 7's
    // comrak-line-aware `blockquote_markers` now allows), `Blockquote`'s
    // own "every marker, then every child" walk order in `walk.rs` no
    // longer matches byte order for that line, and the concatenated
    // rendered text comes out scrambled (verified empirically:
    // `"a\r> q"` rendered as `"> a\rq"` — right bytes, wrong order).
    // Sorting each line's spans by `range`'s start here — the ONE place
    // every producer's output converges before becoming the emitted
    // line — makes "a line's spans are always in byte order" a
    // structural guarantee no producer's own walk order can violate,
    // rather than requiring every current and future producer to get
    // its OWN call order byte-perfect.
    for line_spans in &mut spans {
        line_spans.sort_by_key(|s| s.range().start);
    }

    let lines: Vec<SyntaxLine> = spans
        .into_iter()
        .zip(tables)
        .zip(decors)
        .map(|((spans, table), decor)| SyntaxLine {
            spans,
            table,
            decor,
        })
        .collect();
    let snapshot = SyntaxSnapshot::build(&starts, &hidden);
    (lines, snapshot)
}

#[cfg(test)]
mod decor_tests;
#[cfg(test)]
mod tests;
