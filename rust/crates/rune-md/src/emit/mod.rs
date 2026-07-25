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

mod style;
mod syntax;
mod walk;

pub use style::StyleId;
pub use syntax::{CellMap, SyntaxLine, SyntaxSnapshot, SyntaxSpan};

use crate::element::block::Block;
use crate::element::{ByteRange, RevealState};
use crate::parse::{line_at, line_end_at, line_starts};
use syntax::build_line_conversions;

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
        let Some(text) = content.get(seg_start..seg_end) else {
            return;
        };
        let cell_map = (state == RevealState::Rendered).then(|| build_cell_map(seg_start, text));
        if let Some(bucket) = out.get_mut(line) {
            bucket.push(SyntaxSpan {
                text: text.to_string(),
                style,
                state,
                buffer_start: seg_start,
                buffer_end: seg_end,
                cell_map,
            });
        }
        if let Some(bucket) = accounted.get_mut(line) {
            bucket.push((seg_start, seg_end));
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
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::element::doc::DocMachine;
    use rune_core::buffer::Buffer;
    use rune_core::coords::BufferPoint;
    use rune_core::cursor::CursorSet;

    fn synced(content: &str, cursor_offset: usize, focused: bool) -> (Buffer, DocMachine) {
        let buf = Buffer::new(content);
        let mut doc = DocMachine::new();
        doc.set_focus(focused);
        doc.sync_content(&buf);
        let cursors = CursorSet::new(cursor_offset);
        doc.sync_cursors(&buf, &cursors);
        (buf, doc)
    }

    #[test]
    fn heading_marker_hidden_when_not_on_cursor_line() {
        let (buf, doc) = synced("# hi\nsecond\n", 8, true);
        let (lines, _snap) = emit(buf.content(), doc.blocks());
        let joined: String = lines[0].spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "hi", "marker must be concealed off-cursor-line");
    }

    #[test]
    fn heading_marker_revealed_on_cursor_line() {
        let (buf, doc) = synced("# hi\nsecond\n", 0, true);
        let (lines, _snap) = emit(buf.content(), doc.blocks());
        let joined: String = lines[0].spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "# hi");
    }

    #[test]
    fn unfocused_conceals_everything() {
        let (buf, doc) = synced("# hi\n", 0, false);
        let (lines, _snap) = emit(buf.content(), doc.blocks());
        let joined: String = lines[0].spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "hi");
    }

    #[test]
    fn code_fence_whole_block_reveals_as_unit() {
        let content = "```rust\nfn f() {}\n```\n";
        let (buf, doc) = synced(content, content.find("fn").unwrap(), true);
        let (lines, _snap) = emit(buf.content(), doc.blocks());
        // Cursor is on line 1 (the content line); the whole 3-line fence
        // block must reveal, including the fence marker lines.
        assert_eq!(lines[0].spans[0].text, "```rust");
        assert_eq!(lines[1].spans[0].text, "fn f() {}");
        assert_eq!(lines[2].spans[0].text, "```");
    }

    #[test]
    fn code_fence_conceals_marker_lines_off_cursor() {
        let content = "```rust\nfn f() {}\n```\nafter\n";
        let (buf, doc) = synced(content, content.find("after").unwrap(), true);
        let (lines, _snap) = emit(buf.content(), doc.blocks());
        // Fence marker lines collapse to empty; content line shows verbatim.
        assert_eq!(
            lines[0]
                .spans
                .iter()
                .map(|s| s.text.as_str())
                .collect::<String>(),
            ""
        );
        assert_eq!(lines[1].spans[0].text, "fn f() {}");
    }

    #[test]
    fn bold_reveals_with_nested_link_as_a_unit() {
        let content = "**[bo*ld*](url)** end\n";
        let cursor = content.find("ld").unwrap();
        let (buf, doc) = synced(content, cursor, true);
        let (lines, _snap) = emit(buf.content(), doc.blocks());
        let joined: String = lines[0].spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "**[bo*ld*](url)** end");
    }

    #[test]
    fn bold_conceals_but_still_shows_nested_link_text() {
        let content = "**[bo*ld*](url)** end\n";
        let (buf, doc) = synced(content, content.len(), true); // cursor at " end", not inside bold
        let (lines, _snap) = emit(buf.content(), doc.blocks());
        let joined: String = lines[0].spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "bold end");
    }

    #[test]
    fn rendered_span_cell_map_offsets_are_within_range() {
        let content = "**bold** text\n";
        let (buf, doc) = synced(content, content.len(), true);
        let (lines, _snap) = emit(buf.content(), doc.blocks());
        for span in &lines[0].spans {
            if let Some(cm) = &span.cell_map {
                for &off in cm {
                    assert!(off == -1 || (off as usize) < span.buffer_end);
                    if off != -1 {
                        assert!((off as usize) >= span.buffer_start);
                    }
                }
            }
        }
    }

    #[test]
    fn buffer_to_syntax_roundtrip_on_cursor_legal_position() {
        // Cursor is at end-of-buffer, well outside "**bold**"'s range, so
        // the emphasis is concealed on line 0 and its "**" delimiters (buffer
        // cols [0,2) and [6,8)) are NOT cursor-legal. Buffer col 8 is the
        // space right after the closing "**", a position with no hidden
        // delimiter on either side — genuinely cursor-legal, so the
        // roundtrip must be exact (unlike a position inside a hidden range,
        // which only guarantees the weaker stability invariant the other
        // test below checks).
        let content = "**bold** text\n";
        let (buf, doc) = synced(content, content.len(), true);
        let (_lines, snap) = emit(buf.content(), doc.blocks());
        let bp = BufferPoint { line: 0, col: 8 }; // buffer col 8 = the space after "**bold**"
        let sp = snap.buffer_to_syntax(bp);
        let bp2 = snap.syntax_to_buffer(sp);
        assert_eq!(bp, bp2);
    }

    #[test]
    fn buffer_to_syntax_clamps_stably_inside_hidden_delimiter() {
        // Mirrors Go's FuzzSyntaxMapRoundtrip stability property: a
        // position inside a hidden delimiter range does NOT roundtrip to
        // itself, but the CLAMPED position it lands on must be idempotent.
        let content = "**bold** text\n";
        let (buf, doc) = synced(content, content.len(), true);
        let (_lines, snap) = emit(buf.content(), doc.blocks());
        let bp = BufferPoint { line: 0, col: 0 }; // inside the "**" open delimiter
        let sp = snap.buffer_to_syntax(bp);
        let bp2 = snap.syntax_to_buffer(sp);
        assert_ne!(
            bp, bp2,
            "col 0 sits inside a hidden delimiter, not cursor-legal"
        );
        let sp2 = snap.buffer_to_syntax(bp2);
        assert_eq!(
            sp, sp2,
            "the clamped position must be stable under a second round-trip"
        );
    }
}
