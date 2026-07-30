//! `runtime`'s tree-sitter highlight `Cmd` constructors — split out of
//! `runtime.rs` itself (§1.6 budget) once finding A's per-line source
//! reconstruction and finding B's bounded retry pushed that file over the
//! line limit. Everything here reaches `rune_ts::highlight`; nothing else
//! in `rune-tui` does (see `highlight_cmd`'s own doc comment).

use std::ops::Range;
use std::time::Duration;

use rune_syntax::ScopeId;

use crate::document::DocumentId;
use crate::highlight::FenceLang;

use super::md_fence::markdown_fence_spans;
use super::{Cmd, CmdKind, HighlightPayload, Msg};

/// The wall-clock budget one fence's `rune_ts::highlight` call is allowed
/// before it aborts and reports `None` (plan WP5.S2, Assumption A3) —
/// fence-only since D5: repurposing it for a whole-document parse would
/// silently multiply the per-fence ceiling 20x. Unmeasured against a real
/// large fence (see `TODO.md`).
pub const HIGHLIGHT_BUDGET: Duration = Duration::from_millis(250);

/// The wall-clock budget one whole-document `rune_ts::parse` call is
/// allowed before it aborts and reports `None` (D5, Assumption A2) — one
/// attempt, no retry: a document exceeding this stays plain but now
/// *surfaced* via the status message `dispatch::handle_highlighted` sets on
/// a `None` reply for a scheduled code document, rather than silently
/// uncoloured forever. Unmeasured against a real large document (see
/// `TODO.md`).
pub const PARSE_BUDGET: Duration = Duration::from_secs(5);

/// The wall-clock budget `highlight::first_paint_highlight` gives its single
/// synchronous, pre-first-draw parse attempt at the startup document
/// (Assumption A1) — small on purpose: it runs on the main thread before
/// anything is drawn, so even a miss is invisible (the background `Cmd`
/// picks the document up on the very next frame via the ordinary
/// `schedule_highlight` bootstrap kick) while a hit means frame 1 is already
/// highlighted.
pub(crate) const FIRST_PAINT_BUDGET: Duration = Duration::from_millis(20);

/// Parses `source` as `lang` off-thread — a whole-document parse whose
/// retained `Tree` rides back on `Msg::Highlighted` (D2/D6) — and replies
/// with `Msg::Highlighted`. Owns `source` (moved into the closure, exactly
/// like [`super::load_dir_cmd`]'s owned `root`) since the `Cmd` closure is
/// `FnOnce() -> Option<Msg> + Send + 'static` and cannot borrow the
/// document's buffer across the thread boundary. Always replies with
/// `Some(..)` — even a `None` result from `rune_ts::parse` — so `in_flight`
/// is guaranteed to clear on the UI thread; `rune_ts::parse` itself never
/// panics (§1.3: it surfaces a failed language load or query compile as
/// `None`, never `ts_assert`'s `SIGABRT`, since every parse is a full parse
/// — no incremental-reparse edit is ever fed back into it). This is the
/// ONLY off-thread place `rune-tui` reaches `rune_ts::parse` — the other,
/// sanctioned exception is the single pre-first-draw synchronous attempt in
/// `highlight::first_paint_highlight`.
pub fn highlight_cmd(doc: DocumentId, version: u64, lang: &'static str, source: String) -> Cmd {
    Cmd::new(CmdKind::Highlight, move || {
        let result = rune_ts::parse(lang, &source, PARSE_BUDGET).map(HighlightPayload::Tree);
        Some(Msg::Highlighted {
            doc,
            version,
            result,
        })
    })
}

/// Parses every fence of a markdown document off-thread and merges the
/// results into one reply (plan WP6.S3), wrapped in `Msg::Highlighted`'s
/// `Spans` arm (D6: fences stay on the span path, never a retained tree).
/// `budget` is split evenly across `fences` (`fences.len().max(1)`) so a
/// document with many fences still respects one overall budget rather than
/// running each fence at the full budget.
///
/// Each fence arrives as `(&'static str, Vec<Range<usize>>, String)`: the
/// language, that fence's own per-physical-line buffer ranges
/// (`DocMachine::code_fences`'s per-line output, finding A), and `text` —
/// the PREFIX-FREE source `code_fence_sources` already reconstructed by
/// joining those lines with a single `'\n'`, so a fence nested inside a
/// blockquote or list item never feeds its container's repeating prefix
/// (`"> "`, a list marker's indent) to the parser. Because `text` is no
/// longer a contiguous slice of the buffer for a nested fence, each
/// returned span is mapped back to buffer coordinates through `lines`
/// (`map_reconstructed_span`) rather than a single `base + offset` rebase —
/// a top-level fence's lines are already buffer-contiguous (the gap
/// between two consecutive lines is exactly the buffer's own `'\n'`), so
/// the mapping reduces to that same rebase for it. The concatenated result
/// is re-sorted into the same painter order `rune_ts::highlight` itself
/// guarantees within one fence (`start` ASC, `end` DESC) — concatenating
/// two already-sorted lists is not itself sorted. The return is `Some(..)`
/// iff at least one fence actually parsed within its slice of the budget
/// and `None` iff none did `[R2]` — an all-timed-out document must not
/// flash to unstyled.
fn run_fence_highlight(
    fences: Vec<(FenceLang, Vec<Range<usize>>, String)>,
    budget: Duration,
) -> Option<rune_ts::HighlightResult> {
    let per_fence_budget = budget / (fences.len().max(1) as u32);
    let mut spans: Vec<(Range<usize>, ScopeId)> = Vec::new();
    let mut any_parsed = false;
    // One fence hitting the producer's span cap truncates the whole reply:
    // the document's colours are incomplete either way, and the flag says so.
    let mut truncated = false;
    for (lang, lines, text) in fences {
        // `FenceLang::Markdown` (plan WP6.S4) reuses the comrak emitter
        // instead of `rune_ts::highlight` — a synchronous, bounded parse of
        // the fence's own (typically short) reconstructed text, so it never
        // needs `per_fence_budget`'s timeout; it can only ever add spans, so
        // it never trips `truncated` either.
        let fence_spans = match lang {
            FenceLang::Ts(lang) => {
                let Some(fence) = rune_ts::highlight(lang, &text, per_fence_budget) else {
                    continue;
                };
                any_parsed = true;
                truncated |= fence.truncated;
                fence.spans
            }
            FenceLang::Markdown => {
                any_parsed = true;
                markdown_fence_spans(&text)
            }
        };
        spans.extend(
            fence_spans
                .into_iter()
                .filter_map(|(r, scope)| map_reconstructed_span(&lines, r).map(|r| (r, scope))),
        );
    }
    spans.sort_by(|a, b| a.0.start.cmp(&b.0.start).then(b.0.end.cmp(&a.0.end)));
    any_parsed.then_some(rune_ts::HighlightResult { spans, truncated })
}

pub(crate) fn fence_highlight_cmd(
    doc: DocumentId,
    version: u64,
    fences: Vec<(FenceLang, Vec<Range<usize>>, String)>,
) -> Cmd {
    Cmd::new(CmdKind::Highlight, move || {
        let result = run_fence_highlight(fences, HIGHLIGHT_BUDGET).map(HighlightPayload::Spans);
        Some(Msg::Highlighted {
            doc,
            version,
            result,
        })
    })
}

/// Maps one `rune_ts::highlight` span, given in the coordinates of the
/// prefix-free text `code_fence_sources` reconstructed by joining `lines`
/// (a fence's own per-physical-line buffer ranges) with a single `'\n'`
/// between consecutive lines, back to the real buffer offsets those bytes
/// occupy. A container prefix (a blockquote's `"> "`, a list item's indent)
/// sits in the GAP between two consecutive lines' buffer ranges and
/// contributes nothing to the reconstructed text, so the mapping is
/// piecewise: within one line's own reconstructed span — its content plus
/// the joining `'\n'` that follows it, which is this repo's real buffer
/// newline, never a prefix byte — the shift from reconstructed to buffer
/// offset is constant; it changes only when crossing into the next line.
/// The end of `r` is resolved through the LAST byte it actually covers
/// (`r.end - 1`) rather than `r.end` itself, so an end that lands exactly on
/// a line boundary maps to the position right after that line's own
/// newline — never into the following line's excluded prefix bytes, which
/// is where a naive lookup of `r.end` itself would wander. `None` on any
/// inconsistency (an out-of-range offset, empty `lines`, or an inverted
/// `r`) degrades to "drop this span" — the same silent-skip convention
/// `code_fence_sources` already uses for a fence range that doesn't land on
/// live buffer bytes.
fn map_reconstructed_span(lines: &[Range<usize>], r: Range<usize>) -> Option<Range<usize>> {
    if r.start >= r.end {
        return None;
    }
    let start = map_reconstructed_offset(lines, r.start, false)?;
    let end = map_reconstructed_offset(lines, r.end - 1, true)?;
    if start >= end {
        return None;
    }
    Some(start..end)
}

/// `map_reconstructed_span`'s single-offset chokepoint. `is_end` requests
/// the INCLUSIVE-byte convention that function's `r.end - 1` call needs:
/// after locating the line owning `offset`, add one back so the result is
/// an exclusive buffer offset again, landing right after that line's own
/// content (or its trailing newline, if `offset` was the reconstructed
/// `'\n'` slot) rather than at the start of the next line's prefix.
fn map_reconstructed_offset(lines: &[Range<usize>], offset: usize, is_end: bool) -> Option<usize> {
    let last = lines.len().checked_sub(1)?;
    let mut cursor = 0usize;
    for (i, line) in lines.iter().enumerate() {
        let len = line.end.saturating_sub(line.start);
        // This line's own reconstructed span: its content, plus — for
        // every line but the last — the single joining '\n' that follows
        // it and maps through this SAME line's shift (see the doc comment
        // above: that newline IS the buffer's real line terminator).
        let span_len = if i == last { len } else { len + 1 };
        if offset < cursor + span_len {
            let within = offset - cursor;
            let mapped = line.start + within;
            return Some(if is_end { mapped + 1 } else { mapped });
        }
        cursor += span_len;
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn map_reconstructed_span_is_identity_for_buffer_contiguous_lines() {
        // Top-level fence: consecutive lines are truly adjacent in the
        // buffer (gap of exactly one real '\n'), so the reconstructed text
        // is byte-identical to the buffer slice and every mapped span must
        // equal its own reconstructed-coordinates input verbatim.
        let content = "let a = 1;\nlet b = 2;";
        let lines = vec![0..10, 11..21];
        assert_eq!(content.len(), 21);
        assert_eq!(&content[11..21], "let b = 2;");

        let mapped = map_reconstructed_span(&lines, 11..15).expect("in range");
        assert_eq!(mapped, 11..15);
        assert_eq!(&content[mapped], "let ");
    }

    #[test]
    fn map_reconstructed_span_skips_the_gap_between_nested_lines() {
        // Blockquoted fence: buffer line 1 starts after "> " (2 extra
        // bytes the reconstructed text never sees). A span entirely inside
        // line 1's own reconstructed content must land on line 1's real
        // buffer bytes, never inside the "> " gap.
        let content = "let a = 1;\n> let b = 2;";
        let line0 = 0..10; // "let a = 1;"
        let line1 = 13..23; // "let b = 2;" (after "> ")
        assert_eq!(&content[line1.clone()], "let b = 2;");
        let lines = vec![line0, line1];

        // Reconstructed text: "let a = 1;" + '\n' + "let b = 2;" -> "let"
        // at reconstructed offset 11..14 is line 1's own "let".
        let mapped = map_reconstructed_span(&lines, 11..14).expect("in range");
        assert_eq!(&content[mapped], "let");
    }

    #[test]
    fn map_reconstructed_offset_end_boundary_never_lands_in_the_prefix() {
        let content = "ab\n> cd";
        let line0 = 0..2; // "ab"
        let line1 = 5..7; // "cd" (after "> ")
        let lines = vec![line0, line1];

        // Reconstructed text is "ab\ncd" (5 bytes). A span covering just
        // "ab" plus its joining '\n' (offsets 0..3) must map to the real
        // newline's own end in the buffer (0..3, the byte right after
        // '\n'), never into the "> " gap that follows it.
        let mapped = map_reconstructed_span(&lines, 0..3).expect("in range");
        assert_eq!(mapped, 0..3);
        assert_eq!(&content[mapped], "ab\n");
    }
}
