//! The wrap pass (plan Context, "Emit -> wrap -> snapshot"): a structural
//! port of `pkg/editor/display/wrap_map.go:206-454`. Producer-agnostic
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

pub use query::WrapSnapshot;

use crate::syntax::{SyntaxLine, SyntaxSpan};

/// `ControlAwareWidth` — the single source of truth for a rune's display
/// width, shared (in the Go original) by the wrap/coordinate layer and the
/// cell renderer. Rule: `\n`/`\r` occupy no column; every other rune
/// reported zero-width is clamped to 1 (this is a DISPLAY-width decision
/// only — buffer bytes stay verbatim, §1.4.5).
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

#[derive(Clone, Debug, Default)]
pub struct WrapSegment {
    pub spans: Vec<SyntaxSpan>,
    pub model_line: usize,
    /// Start offset of this segment within its line's syntax-space text, in
    /// BYTES (matches Go's `WrapSegment.StartCol`, which indexes with
    /// `len(text)`).
    pub start_col: usize,
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
        });
    }

    fn wrap_line(
        &self,
        content: &str,
        line_idx: usize,
        line: &SyntaxLine,
        segments: &mut Vec<WrapSegment>,
    ) {
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

        let mut start_col = 0usize;
        while start_col < text.len() {
            let Some(remain) = text.get(start_col..) else {
                break;
            };

            let mut curr_w = 0usize;
            let mut byte_len = 0usize;
            let mut last_space_bytes: Option<usize> = None;

            let mut i = 0usize;
            while let Some(rest) = remain.get(i..) {
                let Some(ch) = rest.chars().next() else {
                    break;
                };
                let size = ch.len_utf8();
                let rw = rune_width_with_tab(ch, curr_w);
                if curr_w + rw > width && byte_len > 0 {
                    break;
                }
                if ch == ' ' || ch == '\t' {
                    last_space_bytes = Some(byte_len + size);
                }
                curr_w += rw;
                byte_len += size;
                i += size;
            }

            if byte_len == 0 && !remain.is_empty() {
                byte_len = remain.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
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
            });

            start_col = seg_end;
        }
    }
}

/// Slice the original spans down to `[seg_start, seg_end)` of the
/// concatenated line text — port of `pkg/editor/display/wrap_map.go`'s
/// `sliceOriginalSpans`. Visible text is sliced identically for both
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
            SyntaxSpan::Identical { style, range } => SyntaxSpan::Identical {
                style: *style,
                range: (range.start + local_start)..(range.start + local_end),
            },
            // `range` intentionally left as the full original span range
            // (Go parity, module docs); `cell_map` is rune-sliced to match
            // `sliced` instead.
            SyntaxSpan::Substituted {
                style,
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
                    style: *style,
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
    use crate::style::StyleId;
    use crate::syntax::CellMap;

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
                            style: StyleId::Text,
                            range: s..line_end,
                        }],
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
                    style: StyleId::Text,
                    range: 0..2,
                },
                SyntaxSpan::Substituted {
                    style: StyleId::Code,
                    text: code_text.to_string(),
                    range: 3..23,
                    cell_map,
                },
                SyntaxSpan::Identical {
                    style: StyleId::Text,
                    range: 24..26,
                },
            ],
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
}
