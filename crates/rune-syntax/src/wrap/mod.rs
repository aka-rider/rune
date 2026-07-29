//! The wrap pass (plan Context, "Emit -> wrap -> snapshot"): a structural
//! port of Go's wrap map. Producer-agnostic
//! (WP3): consumes whatever `SyntaxLine`s a producer emitted — `rune-md`'s
//! `DocMachine::snapshot` is the only caller today ("children never wrap
//! themselves"), but nothing here depends on markdown.
//!
//! A `Substituted` (concealed) span's visible TEXT is not byte-for-byte
//! aligned to its BUFFER `range` (delimiters were dropped), so a
//! `Substituted` span's `range` cannot be narrowed when the greedy break
//! lands inside it — but its `text` and `cell_map` CAN be, and are: this is
//! Go's actual behavior (`wrap_map.go:411-424`), not the "include whole,
//! never split" reading its own comment there suggests. The comment
//! describes why `BufferStart`/`BufferEnd` stay untouched; the code two
//! lines below it still does `s.Text[localStart:localEnd]` and
//! rune-slices `CellMap` to match. Ported literally: `slice_spans` below
//! slices `Substituted` spans exactly like `Identical` ones for `text`,
//! and slices `cell_map` by RUNE count into that same byte range, while
//! leaving `range` at the span's full original value — the `cell_map`,
//! not the buffer range, is the authoritative per-char mapping for a
//! `Substituted` span from here on.
//!
//! Split in two (CONSTITUTION §1.6) along the seam the wrap pass already
//! has: this file computes wrap segments (`WrapMap`, `wrap_line`,
//! `slice_spans`); `query.rs` answers coordinate questions about them
//! (`WrapSnapshot`'s `syntax_to_wrap`/`wrap_to_syntax`/`visual_col`/
//! `byte_col_from_visual`/etc). `WrapSegment` is defined here, where it's
//! produced, and read by both halves.

mod query;
mod table;

pub use query::WrapSnapshot;

use unicode_segmentation::UnicodeSegmentation;

pub use table::TableSegInfo;

use crate::syntax::{SyntaxLine, SyntaxSpan};

/// `ControlAwareWidth` — the single source of truth for a rune's display
/// width, shared (in the Go original) by the wrap/coordinate layer and the
/// cell renderer. Rule: `\n`/`\r` occupy no column; every other rune
/// reported zero-width is clamped to 1 (this is a DISPLAY-width decision
/// only — buffer bytes stay verbatim, §1.4.5). The 1-clamp exists so a LONE
/// zero-width rune (an isolated control char, a bare combining mark with no
/// base) still gets its own reachable caret column — see `grapheme_width`
/// below for why that reasoning does NOT extend to a rune that's part of a
/// larger grapheme cluster.
pub fn control_aware_width(r: char) -> usize {
    if r == '\n' || r == '\r' {
        return 0;
    }
    match unicode_width::UnicodeWidthChar::width(r) {
        Some(w) if w > 0 => w,
        _ => 1,
    }
}

/// `runeWidthWithTab`: a tab expands to the next multiple-of-4 stop.
pub fn rune_width_with_tab(r: char, current_width: usize) -> usize {
    if r == '\t' {
        return 4 - (current_width % 4);
    }
    control_aware_width(r)
}

/// The display width of a WHOLE grapheme cluster (`unicode_segmentation::
/// graphemes`) — the second half of the shared width chokepoint alongside
/// `control_aware_width`/`rune_width_with_tab` above, used by BOTH
/// `wrap_line`'s greedy line-breaking (below) and `rune-tui`'s
/// `render::push_grapheme_cells`/`WrapSnapshot::visual_col`/
/// `byte_col_from_visual` (`query.rs`) — every place that walks text one
/// visual unit at a time, not one rune at a time, must agree on this
/// number or the caret lands on the wrong cell (this module's own
/// load-bearing property).
///
/// A single-rune cluster (plain ASCII, CJK, an isolated control char — the
/// overwhelming common case) delegates to `control_aware_width` unchanged.
///
/// A MULTI-rune cluster (a ZWJ-joined emoji sequence, a skin-tone-modified
/// emoji, a base char plus a combining mark) takes the MAX of each rune's
/// RAW `unicode_width::UnicodeWidthChar::width` — never the SUM, and never
/// `control_aware_width`'s 1-clamp per rune. Both would overcount: a real
/// terminal renders the WHOLE cluster as one glyph occupying as many
/// columns as its single widest member needs (confirmed empirically
/// against a real tmux capture, `scripts/parity/fixtures/emoji.md` — a
/// summed or per-rune-clamped width reserves MORE columns than the
/// terminal actually consumes, leaving a visible gap of blank columns
/// before whatever follows on the row, the residual divergence a first
/// attempt at this fix left behind). `control_aware_width`'s 1-clamp exists
/// so a LONE zero-width rune still gets a reachable caret column, which
/// doesn't apply inside a cluster either — it already has exactly one
/// caret position, at its first byte, regardless of how many joiner/
/// modifier/combining runes follow; a joiner contributing 0 to the MAX is
/// correct precisely because the cluster's OWN width comes from its widest
/// member, not from that rune. The result is still clamped to a minimum of
/// 1 overall (never zero), so a real buffer byte always renders as at
/// least one cell even for the pathological all-zero-width cluster
/// (`CELL-OFFSET`'s invariant, `rune-fuzz`).
pub fn grapheme_width(cluster: &str) -> usize {
    let mut chars = cluster.chars();
    let Some(first) = chars.next() else {
        return 0;
    };
    if chars.next().is_none() {
        return control_aware_width(first);
    }
    cluster
        .chars()
        .filter_map(unicode_width::UnicodeWidthChar::width)
        .max()
        .unwrap_or(0)
        .max(1)
}

/// `grapheme_width`'s tab-aware counterpart, mirroring `rune_width_with_tab`
/// — a tab is always its own single-rune cluster (grapheme segmentation
/// never joins a control char to a neighboring rune), so the two functions
/// agree exactly on every non-tab cluster and this only adds the 4-stop
/// tab-expansion case on top.
pub fn grapheme_width_with_tab(cluster: &str, current_width: usize) -> usize {
    if cluster == "\t" {
        return 4 - (current_width % 4);
    }
    grapheme_width(cluster)
}

/// The next grapheme cluster in a row's concatenated span text, starting at
/// byte `pos`, clamped to never read past `bounds`' nearest entry greater
/// than `pos` (each entry is one `SyntaxSpan`'s own end offset within the
/// concatenation, ascending, last entry == `text.len()`).
///
/// This is the ONE place every width/column walk over a whole row's text
/// draws its cluster boundaries — `wrap_line`'s greedy breaker below and
/// the coordinate queries in this module's sibling all call it rather than
/// re-deriving boundaries via a bare `graphemes(true)` over the
/// concatenation. It exists because the code that actually decides what lands in
/// which terminal `Cell` grapheme-segments each span's own text
/// INDEPENDENTLY — a cluster's
/// `buf_offset`/style/`cell_map` lookup comes from exactly one span, never
/// two, so a cluster can never legitimately straddle a span boundary there.
/// A bare `graphemes(true)` walk over spans concatenated first has no such
/// boundary and will happily fuse characters across the seam: Unicode's own
/// cluster-break rules join a ZWJ (or any combining/extending rune) to
/// WHATEVER precedes it, span boundary or not (UAX #29 GB9 — unconditional,
/// unlike the pictographic-specific GB11 continuation rule the ZWJ sequence
/// itself relies on). Left unclamped, that fusion makes this module's width
/// sum diverge from the row's actual cells by exactly the fused cluster's
/// width whenever a span boundary happens to land right before such a rune
/// — silently, since both sides still individually look correct — and the
/// resulting column mismatch is what let a cursor's computed visual column
/// fail to land on any real cell (`CELL-ORDER`, `rune-fuzz`).
pub(super) fn next_grapheme<'a>(text: &'a str, bounds: &[usize], pos: usize) -> Option<&'a str> {
    let limit = bounds
        .iter()
        .copied()
        .find(|&b| b > pos)
        .unwrap_or(text.len());
    text.get(pos..limit)?.graphemes(true).next()
}

#[derive(Clone, Debug, Default)]
pub struct WrapSegment {
    pub spans: Vec<SyntaxSpan>,
    pub model_line: usize,
    /// Start offset of this segment within its line's syntax-space text, in
    /// BYTES (matches Go's `WrapSegment.StartCol`, which indexes with
    /// `len(text)`).
    pub start_col: usize,
    /// `Some` only for a segment that came from a table source line —
    /// `None` for every other segment, prose included.
    pub table: Option<TableSegInfo>,
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
        segments.push(WrapSegment {
            spans: line.spans.clone(),
            model_line: line_idx,
            start_col: 0,
            table: None,
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

        // (span index, start offset, end offset) in the concatenated line
        // text.
        let mut span_refs: Vec<(usize, usize, usize)> = Vec::with_capacity(line.spans.len());
        let mut text = String::new();
        for (i, s) in line.spans.iter().enumerate() {
            let start = text.len();
            let span_text = s.text(content);
            text.push_str(span_text);
            span_refs.push((i, start, start + span_text.len()));
        }

        if text.is_empty() {
            self.push_whole_line(line_idx, line, segments);
            return;
        }

        // Same coordinate space as `span_refs`' end offsets — the boundary
        // list `next_grapheme` clamps cluster reads to, so this loop's width
        // sum can never silently disagree with the row the renderer
        // actually builds cells for (this function's own docs).
        let bounds: Vec<usize> = span_refs.iter().map(|&(_, _, end)| end).collect();

        let mut start_col = 0usize;
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
                if curr_w + rw > width && byte_len > 0 {
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
                byte_len = next_grapheme(&text, &bounds, start_col)
                    .map(str::len)
                    .unwrap_or(1);
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

            let seg_spans = slice_spans(content, &line.spans, &span_refs, seg_start, seg_end);
            segments.push(WrapSegment {
                spans: seg_spans,
                model_line: line_idx,
                start_col: seg_start,
                table: None,
            });

            start_col = seg_end;
        }
    }
}

/// Slice the original spans down to `[seg_start, seg_end)` of the
/// concatenated line text — port of Go's `sliceOriginalSpans`.
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
    span_refs: &[(usize, usize, usize)],
    seg_start: usize,
    seg_end: usize,
) -> Vec<SyntaxSpan> {
    let mut result = Vec::new();
    for &(idx, start_off, end_off) in span_refs {
        if end_off <= seg_start || start_off >= seg_end {
            continue;
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
            // (Go parity, module docs); `cell_map` is rune-sliced to match
            // `sliced` instead.
            SyntaxSpan::Substituted {
                scope,
                range,
                cell_map,
                ..
            } => {
                let start_runes = full_text
                    .get(..local_start)
                    .map(|p| p.chars().count())
                    .unwrap_or(0);
                let end_runes = start_runes + sliced.chars().count();
                SyntaxSpan::Substituted {
                    scope: *scope,
                    text: sliced.to_string(),
                    range: range.clone(),
                    cell_map: cell_map
                        .get(start_runes..end_runes)
                        .map(<[i64]>::to_vec)
                        .unwrap_or_default(),
                }
            }
        };
        result.push(out);
    }
    result
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::scope::ScopeId;
    use crate::syntax::CellMap;

    const TEXT: ScopeId = ScopeId(0);
    const CODE: ScopeId = ScopeId(1);

    /// Splits `content` on `\n` into one `Identical`, whole-line `SyntaxLine`
    /// per line (dropping the line terminator from the visible range, an
    /// empty line becoming a `SyntaxLine::default()`) — a minimal stand-in
    /// for a producer's emitted output. Builds this crate's own test inputs
    /// directly rather than routing through `rune-md`'s `DocMachine`/`emit`
    /// (WP3: `rune-syntax` must stand up without depending on `rune-md`);
    /// these tests exercise `WrapMap`'s own contract only, not concealment.
    fn plain_lines(content: &str) -> Vec<SyntaxLine> {
        let mut starts = vec![0usize];
        for (i, b) in content.bytes().enumerate() {
            if b == b'\n' {
                starts.push(i + 1);
            }
        }
        starts
            .iter()
            .enumerate()
            .map(|(i, &s)| {
                let e = starts.get(i + 1).copied().unwrap_or(content.len());
                let line_end = if e > s && content.as_bytes().get(e - 1) == Some(&b'\n') {
                    e - 1
                } else {
                    e
                };
                if line_end > s {
                    SyntaxLine {
                        spans: vec![SyntaxSpan::Identical {
                            scope: TEXT,
                            range: s..line_end,
                        }],
                        table: None,
                    }
                } else {
                    SyntaxLine::default()
                }
            })
            .collect()
    }

    fn wrap_lines(content: &str, width: u16) -> WrapSnapshot {
        let lines = plain_lines(content);
        WrapMap::new(width).sync(content, &lines)
    }

    #[test]
    fn short_line_is_a_single_segment() {
        let wrap = wrap_lines("hello world\n", 80);
        assert_eq!(wrap.total_rows(), 2); // "hello world" + the trailing empty line
        assert_eq!(wrap.segment_len_at(0), 11);
    }

    #[test]
    fn long_line_breaks_before_width_limit_at_the_last_space_seen() {
        // Go's greedy loop (wrap_map.go:316-343) always backs off to the
        // last space it has seen so far whenever more text remains past the
        // width-fitting cutoff — even when the width-fitting cutoff itself
        // lands cleanly at a word boundary. width=11 fits "hello world"
        // (11 cols) exactly, but "again" still remains, so the segment
        // backs off to right after the FIRST space: "hello ".
        let content = "hello world again\n";
        let wrap = wrap_lines(content, 11);
        let seg0 = &wrap.segments()[0];
        let text: String = seg0.spans.iter().map(|s| s.text(content)).collect();
        assert_eq!(text, "hello ");

        // No segment on this line exceeds the configured width, and the
        // segments concatenate back to the exact original line text.
        let line0_segments: Vec<&WrapSegment> = wrap
            .segments()
            .iter()
            .filter(|s| s.model_line == 0)
            .collect();
        let mut joined = String::new();
        for seg in &line0_segments {
            let seg_text: String = seg.spans.iter().map(|s| s.text(content)).collect();
            joined.push_str(&seg_text);
        }
        assert_eq!(joined, "hello world again");
    }

    #[test]
    fn rendered_span_text_and_cell_map_split_together_buffer_range_stays_whole() {
        // Go parity (module docs): a Substituted span's TEXT and CellMap DO
        // get sliced at a wrap break, same as any other span — only its
        // `range` is left at the full original range, because a
        // Substituted span's text isn't byte-for-byte its buffer range once
        // delimiters are dropped. Hand-built (see `plain_lines`'s docs): a
        // concealed inline-code span, its delimiting backticks NOT part of
        // any span's range (they'd be a separate hidden range in a real
        // producer's `SyntaxSnapshot`, irrelevant to `WrapMap`).
        let content = "x `aaaaaaaaaaaaaaaaaaaa` y\n";
        let code_text = "aaaaaaaaaaaaaaaaaaaa";
        let cell_map: CellMap = (3..3 + code_text.len() as i64).collect();
        let line0 = SyntaxLine {
            spans: vec![
                SyntaxSpan::Identical {
                    scope: TEXT,
                    range: 0..2,
                },
                SyntaxSpan::Substituted {
                    scope: CODE,
                    text: code_text.to_string(),
                    range: 3..23,
                    cell_map,
                },
                SyntaxSpan::Identical {
                    scope: TEXT,
                    range: 24..26,
                },
            ],
            table: None,
        };
        let wrap = WrapMap::new(6).sync(content, &[line0]);

        let mut full_rendered_text = String::new();
        let mut buffer_ranges: Vec<(usize, usize)> = Vec::new();
        for seg in wrap.segments().iter().filter(|s| s.model_line == 0) {
            for sp in &seg.spans {
                let text = sp.text(content);
                assert!(
                    text.chars().map(control_aware_width).sum::<usize>() <= 6
                        || text.chars().count() == 1,
                    "segment exceeds width 6 without being a single over-wide char: {text:?}",
                );
                if sp.is_rendered() {
                    full_rendered_text.push_str(text);
                    let r = sp.range();
                    buffer_ranges.push((r.start, r.end));
                }
            }
        }
        // The concealed inline-code content is split across multiple
        // segments (width 6 can't fit all 20 'a's on one row) but
        // reconstructs exactly, and every piece's buffer range is the
        // SAME full original span range (Go parity — never narrowed).
        assert_eq!(full_rendered_text, "aaaaaaaaaaaaaaaaaaaa");
        assert!(
            buffer_ranges.len() > 1,
            "expected the rendered span to be split across more than one segment"
        );
        let first = buffer_ranges[0];
        for r in &buffer_ranges {
            assert_eq!(
                *r, first,
                "the span's range must stay at the full original range on every slice"
            );
        }
    }

    /// `CELL-ORDER` regression: a `Substituted` span (a
    /// concealed link's visible text, same shape `rune-md`'s emitter
    /// produces) immediately followed by an `Identical` span whose text
    /// starts with a LONE zero-width joiner — exactly what both
    /// the two checked-in replay repros reduce to (a ZWJ family
    /// emoji pasted right after concealed/marker text, then edited until
    /// the emoji's own leading base codepoint is gone, leaving the visible
    /// text starting on a bare ZWJ). The renderer's actual `Cell` layout
    /// ALWAYS grapheme-segments each span's own text independently,
    /// span by span: the substituted `"a"` never
    /// joins to the ZWJ starting the next span, because that span's text
    /// is segmented on its own, starting fresh — two single-width cells,
    /// not one fused cluster (a lone ZWJ has nothing to join to at the
    /// start of a string). `visual_col`/`byte_col_from_visual` must agree,
    /// or a cursor's computed column stops lining up with any real `Cell`
    /// — the caret placer's "no matching column" fallback then
    /// appends a synthetic caret cell at the row's END, out of
    /// `buf_offset` order, which is the actual `CELL-ORDER` failure both
    /// repros hit.
    #[test]
    fn visual_col_does_not_fuse_a_zwj_across_a_span_boundary() {
        let content = "a\u{200d}\u{1f469}"; // "a" + ZWJ + 👩
        let spans = vec![
            SyntaxSpan::Substituted {
                scope: TEXT,
                text: "a".to_string(),
                range: 0..1,
                cell_map: vec![0],
            },
            SyntaxSpan::Identical {
                scope: TEXT,
                range: 1..content.len(),
            },
        ];

        // Per-span segmentation (what the renderer actually builds): "a"
        // (width 1), then the SECOND span segmented on its own — its text
        // starts fresh, so the ZWJ has no preceding char to join to and
        // stands as its own cluster (width 1) — then the emoji (width 2).
        // Total: 1 + 1 + 2 = 4. A concatenated-then-segmented walk instead
        // fuses "a" and the ZWJ into one cluster (UAX #29 GB9 joins a ZWJ
        // to WHATEVER precedes it, unconditionally) and undercounts: 1 + 2
        // = 3.
        let end = query::visual_col(content, &spans, content.len());
        assert_eq!(
            end, 4,
            "a span boundary must force a grapheme-cluster break even \
             before a ZWJ — got {end}, which is what fusing \"a\" and the \
             ZWJ into one cluster produces instead of keeping them separate"
        );

        // Round-trips: the byte offset for that same visual column is the
        // whole content, never somewhere mid-cluster.
        assert_eq!(
            query::byte_col_from_visual(content, &spans, end),
            content.len()
        );
    }
}
