//! The wrap pass (plan Context, "Emit -> wrap -> snapshot"). Producer-agnostic
//! (WP3): consumes whatever `SyntaxLine`s a producer emitted — `rune-md`'s
//! `DocMachine::snapshot` is the only caller today ("children never wrap
//! themselves"), but nothing here depends on markdown.
//!
//! A `Substituted` (concealed) span's visible TEXT is not byte-for-byte
//! aligned to its BUFFER `range` (delimiters were dropped), so a
//! `Substituted` span's `range` cannot be narrowed when the greedy break
//! lands inside it — but its `text` and `cell_map` CAN be, and are:
//! `slice_spans` below slices `Substituted` spans exactly like `Identical`
//! ones for `text`, and slices `cell_map` by RUNE count into that same byte
//! range, while leaving `range` at the span's full original value — the
//! `cell_map`, not the buffer range, is the authoritative per-char mapping
//! for a `Substituted` span from here on.
//!
//! Split in two along the seam the wrap pass already has: this module
//! computes wrap segments (`WrapMap`, `wrap_line`,
//! `slice_spans`); the sibling `query` submodule answers coordinate
//! questions about them (`WrapSnapshot`'s `syntax_to_wrap`/`wrap_to_syntax`/
//! `visual_col`/`byte_col_from_visual`/etc). `WrapSegment` is defined here,
//! where it's produced, and read by both halves.

mod decor;
mod query;
mod table;
mod width;

pub use decor::{SegDecor, SegDecorPiece};
pub use query::{WrapSnapshot, line_visual_col};

pub use table::TableSegInfo;
pub use width::{
    TAB_STOP, control_aware_width, grapheme_width, grapheme_width_with_tab, rune_width_with_tab,
};

use width::next_grapheme;

use crate::syntax::{SyntaxLine, SyntaxSpan};
use rune_core::assert_invariant;

#[derive(Clone, Debug, Default)]
pub struct WrapSegment {
    pub spans: Vec<SyntaxSpan>,
    pub model_line: usize,
    /// Start offset of this segment within its line's syntax-space text, in
    /// BYTES.
    pub start_col: usize,
    /// `Some` only for a segment that came from a table source line —
    /// `None` for every other segment, prose included.
    pub table: Option<TableSegInfo>,
    /// This segment's own rendered decoration (heading icon, list bullet,
    /// quote bar, hr rule) — sibling to `table`, never inside `spans`, so
    /// the query submodule's byte-exact span walks never have to know decor
    /// exists. `None` for a line with no `SyntaxLine::decor`, or when the
    /// decor didn't fit and (not being a rule) was dropped — see
    /// `decor::attach`.
    pub decor: Option<SegDecor>,
}

pub struct WrapMap {
    width: u16,
}

impl WrapMap {
    pub fn new(width: u16) -> WrapMap {
        WrapMap { width }
    }

    pub fn sync(&self, content: &str, lines: &[SyntaxLine]) -> WrapSnapshot {
        let mut segments: Vec<WrapSegment> = Vec::new();
        let mut line_to_first_row = vec![0usize; lines.len()];

        for (line_idx, line) in lines.iter().enumerate() {
            if let Some(slot) = line_to_first_row.get_mut(line_idx) {
                *slot = segments.len();
            }
            self.wrap_line(content, line_idx, line, &mut segments);
        }

        WrapSnapshot::new(segments, line_to_first_row)
    }

    fn push_whole_line(&self, line_idx: usize, line: &SyntaxLine, segments: &mut Vec<WrapSegment>) {
        // The only path an hr's `LineDecor` (no visible spans of its own)
        // ever takes — module docs, WP3.S3 — so decor must attach here too,
        // not just in `wrap_line`'s greedy loop below.
        let seg_decor = decor::attach(
            line.decor.as_ref(),
            decor::SegmentPosition::First,
            self.width as usize,
        );
        segments.push(WrapSegment {
            spans: line.spans.clone(),
            model_line: line_idx,
            start_col: 0,
            table: None,
            decor: seg_decor,
        });
    }

    fn wrap_line(
        &self,
        content: &str,
        line_idx: usize,
        line: &SyntaxLine,
        segments: &mut Vec<WrapSegment>,
    ) {
        if let Some(info) = &line.table {
            table::wrap_table_line(line_idx, line, info, segments);
            return;
        }
        if line.spans.is_empty() || self.width == 0 {
            self.push_whole_line(line_idx, line, segments);
            return;
        }
        let width = self.width as usize;

        let (text, bounds) = query::spans_text_and_bounds(content, &line.spans);

        if text.is_empty() {
            self.push_whole_line(line_idx, line, segments);
            return;
        }

        // How much of `width` the greedy breaker below actually has to lay
        // content out in — reduced when `line.decor` reserves cells for
        // itself (heading icon / bullet / quote bar), unchanged when there
        // is none, when it doesn't fit and will be dropped, or when it's a
        // rule (which has no competing content, module docs).
        let content_width = decor::content_budget(line.decor.as_ref(), width);

        let mut start_col = 0usize;
        let mut seg_index = 0usize;
        while start_col < text.len() {
            let Some(remain) = text.get(start_col..) else {
                break;
            };

            let mut curr_w = 0usize;
            let mut byte_len = 0usize;
            let mut last_space_bytes: Option<usize> = None;

            let mut i = 0usize;
            while let Some(cluster) = next_grapheme(&text, &bounds, start_col + i) {
                let size = cluster.len();
                let rw = grapheme_width_with_tab(cluster, curr_w);
                if curr_w + rw > content_width && byte_len > 0 {
                    break;
                }
                if cluster == " " || cluster == "\t" {
                    last_space_bytes = Some(byte_len + size);
                }
                curr_w += rw;
                byte_len += size;
                i += size;
            }

            if byte_len == 0 && !remain.is_empty() {
                byte_len = next_grapheme(&text, &bounds, start_col).map_or(1, str::len);
            } else if byte_len < remain.len()
                && let Some(sp) = last_space_bytes
                && sp > 0
            {
                byte_len = sp;
            }

            let seg_start = start_col;
            // `byte_len >= 1` always (the force-include-one-char fallback
            // above guarantees it), so the loop always progresses.
            let seg_end = (start_col + byte_len).min(text.len());

            let seg_spans = slice_spans(content, &line.spans, &bounds, seg_start, seg_end);
            let position = if seg_index == 0 {
                decor::SegmentPosition::First
            } else {
                decor::SegmentPosition::Continuation
            };
            let seg_decor = decor::attach(line.decor.as_ref(), position, width);
            segments.push(WrapSegment {
                spans: seg_spans,
                model_line: line_idx,
                start_col: seg_start,
                table: None,
                decor: seg_decor,
            });

            start_col = seg_end;
            seg_index += 1;
        }
    }
}

/// Slice the original spans down to `[seg_start, seg_end)` of the
/// concatenated line text.
/// Visible text is sliced identically for both
/// variants; only the buffer-range metadata differs (module docs):
/// `Identical` re-bases `range` to match (its text IS byte-for-byte its
/// buffer range, so slicing narrows `range` too); `Substituted` leaves
/// `range` at the span's full original value and rune-slices `cell_map` —
/// the authoritative per-char mapping for a `Substituted` span — to match
/// instead (its text is NOT byte-for-byte its buffer range once
/// delimiters are dropped).
fn slice_spans(
    content: &str,
    spans: &[SyntaxSpan],
    bounds: &[usize],
    seg_start: usize,
    seg_end: usize,
) -> Vec<SyntaxSpan> {
    let mut result = Vec::new();
    // `bounds` is built by one forward pass over the line's spans (this
    // module's `wrap_line`), so end offsets rise monotonically — the first
    // span able to reach `seg_start` is found by binary search, and once a
    // span starts at or past `seg_end` every later one does too.
    let first = bounds.partition_point(|&end_off| end_off <= seg_start);
    for idx in first..spans.len() {
        let start_off = idx
            .checked_sub(1)
            .and_then(|prev| bounds.get(prev))
            .copied()
            .unwrap_or(0);
        if start_off >= seg_end {
            break;
        }
        let Some(s) = spans.get(idx) else {
            continue;
        };
        let full_text = s.text(content);
        let local_start = seg_start.saturating_sub(start_off).min(full_text.len());
        let local_end = seg_end.saturating_sub(start_off).min(full_text.len());
        let Some(sliced) = (local_end > local_start)
            .then(|| full_text.get(local_start..local_end))
            .flatten()
        else {
            continue;
        };

        let out = match s {
            SyntaxSpan::Identical { scope, range } => SyntaxSpan::Identical {
                scope: *scope,
                range: (range.start + local_start)..(range.start + local_end),
            },
            // `range` intentionally left as the full original span range
            // (see module docs); `cell_map` is rune-sliced to match
            // `sliced` instead.
            SyntaxSpan::Substituted {
                scope,
                range,
                cell_map,
                ..
            } => {
                let start_runes = full_text
                    .get(..local_start)
                    .map_or(0, |p| p.chars().count());
                let end_runes = start_runes + sliced.chars().count();
                // Clamp rather than discard the whole map on a length
                // mismatch: a one-entry-short `cell_map`
                // used to cost the entire span (`unwrap_or_default()` ->
                // every char in it became caret-unreachable via the `None`
                // no-correspondence sentinel). Clamping keeps every
                // in-bounds mapping the producer DID supply; only the
                // genuinely missing tail is lost, and the mismatch itself
                // still surfaces via `assert_invariant` (test-only).
                let cm_len = cell_map.len();
                let clamped_start = start_runes.min(cm_len);
                let clamped_end = end_runes.min(cm_len).max(clamped_start);
                assert_invariant!(
                    clamped_start == start_runes && clamped_end == end_runes,
                    || {
                        format!(
                            "cell_map length {cm_len} disagrees with the sliced text's rune range [{start_runes},{end_runes}) — producer bug; clamped to [{clamped_start},{clamped_end})"
                        )
                    },
                );
                SyntaxSpan::substituted_mapped(
                    *scope,
                    sliced.to_string(),
                    range.clone(),
                    // `get` + `unwrap_or_default` rather than a raw index:
                    // the clamp above guarantees this `Some`s, but the
                    // fallback stays as defense-in-depth rather than an
                    // indexing panic if that guarantee is ever
                    // violated.
                    cell_map
                        .get(clamped_start..clamped_end)
                        .map(<[Option<u32>]>::to_vec)
                        .unwrap_or_default(),
                )
            }
        };
        result.push(out);
    }
    result
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
#[path = "wrap_tests.rs"]
mod tests;
