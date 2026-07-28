//! The wrap pass (plan Context, "Emit -> wrap -> snapshot"): a structural
//! port of `pkg/editor/display/wrap_map.go:206-454`. Runs only inside
//! `DocMachine::snapshot` — children never wrap themselves.
//!
//! A `Substituted` (concealed) span's visible text is not byte-for-byte
//! aligned to its buffer `range` (delimiters were dropped), so `range`
//! cannot be narrowed when a break lands inside it — but its `text` and
//! `cell_map` CAN be, and are (Go parity, `wrap_map.go:411-424`):
//! `slice_spans` below slices `Substituted` exactly like `Identical` for
//! visible text, and slices `cell_map` by RUNE count into that byte range,
//! leaving `range` at the span's full original value.

use crate::emit::{SyntaxLine, SyntaxSpan};
use rune_core::coords::{SyntaxPoint, WrapPoint};

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
    /// BYTES (matches Go's `WrapSegment.StartCol`).
    pub start_col: usize,
}

#[derive(Clone, Debug, Default)]
pub struct WrapSnapshot {
    // Phase 1 has no table/image row expansion, so `segments` IS the row
    // index — row `i` is always `segments[i]`.
    segments: Vec<WrapSegment>,
    line_to_first_row: Vec<usize>,
}

impl WrapSnapshot {
    fn clamp_line(&self, line: usize) -> Option<usize> {
        if self.line_to_first_row.is_empty() {
            return None;
        }
        Some(line.min(self.line_to_first_row.len() - 1))
    }

    fn clamp_row(&self, row: usize) -> Option<usize> {
        if self.segments.is_empty() {
            return None;
        }
        Some(row.min(self.segments.len() - 1))
    }

    fn segment_at(&self, row: usize) -> Option<&WrapSegment> {
        let row = self.clamp_row(row)?;
        self.segments.get(row)
    }

    // `content` is the caller-supplied buffer text this snapshot was built
    // from (WP2 fix: previously an owned `content: String` field on
    // `WrapSnapshot` itself — an O(document) allocation and copy on every
    // wrap sync — recovered it instead; that field is gone, so every query
    // that needs an `Identical` span's text now takes `content: &str` from
    // its caller, exactly like `render::segment_cells` already does).
    fn segment_text(&self, content: &str, seg: &WrapSegment) -> String {
        if let [only] = seg.spans.as_slice() {
            return only.text(content).to_string();
        }
        let mut s = String::new();
        for sp in &seg.spans {
            s.push_str(sp.text(content));
        }
        s
    }

    fn segment_len(seg: &WrapSegment) -> usize {
        seg.spans.iter().map(span_visible_len).sum()
    }

    pub fn syntax_to_wrap(&self, sp: SyntaxPoint) -> WrapPoint {
        let Some(line) = self.clamp_line(sp.line) else {
            return WrapPoint { row: 0, col: 0 };
        };
        let first_row = self.line_to_first_row.get(line).copied().unwrap_or(0);

        let mut last_seg_row = first_row;
        let mut last_seg_len = 0usize;
        let mut i = first_row;
        while let Some(seg) = self.segments.get(i) {
            if seg.model_line != line {
                break;
            }
            let seg_len = Self::segment_len(seg);
            last_seg_row = i;
            last_seg_len = seg_len;

            let next_is_same_line = self.segments.get(i + 1).map(|s| s.model_line) == Some(line);
            let upper_inclusive = !next_is_same_line;

            let fits = if upper_inclusive {
                sp.col >= seg.start_col && sp.col <= seg.start_col + seg_len
            } else {
                sp.col >= seg.start_col && sp.col < seg.start_col + seg_len
            };
            if fits {
                return WrapPoint {
                    row: i,
                    col: sp.col.saturating_sub(seg.start_col),
                };
            }
            i += 1;
        }

        WrapPoint {
            row: last_seg_row,
            col: last_seg_len,
        }
    }

    pub fn wrap_to_syntax(&self, wp: WrapPoint) -> SyntaxPoint {
        let Some(seg) = self.segment_at(wp.row) else {
            return SyntaxPoint { line: 0, col: 0 };
        };
        let seg_len = Self::segment_len(seg);
        let col = wp.col.min(seg_len);
        SyntaxPoint {
            line: seg.model_line,
            col: seg.start_col + col,
        }
    }

    pub fn segment_len_at(&self, row: usize) -> usize {
        self.segment_at(row).map(Self::segment_len).unwrap_or(0)
    }

    pub fn visual_col(&self, content: &str, row: usize, byte_col: usize) -> usize {
        let Some(seg) = self.segment_at(row) else {
            return 0;
        };
        let text = self.segment_text(content, seg);
        let mut visual = 0usize;
        let mut bytes = 0usize;
        while bytes < text.len() && bytes < byte_col {
            let Some(ch) = text.get(bytes..).and_then(|s| s.chars().next()) else {
                break;
            };
            visual += rune_width_with_tab(ch, visual);
            bytes += ch.len_utf8();
        }
        visual
    }

    pub fn byte_col_from_visual(&self, content: &str, row: usize, visual_col: usize) -> usize {
        let Some(seg) = self.segment_at(row) else {
            return 0;
        };
        let text = self.segment_text(content, seg);
        let mut visual = 0usize;
        let mut bytes = 0usize;
        while bytes < text.len() {
            let Some(ch) = text.get(bytes..).and_then(|s| s.chars().next()) else {
                break;
            };
            let rw = rune_width_with_tab(ch, visual);
            if visual + rw > visual_col {
                break;
            }
            visual += rw;
            bytes += ch.len_utf8();
        }
        bytes
    }

    pub fn model_line_to_first_row(&self, line: usize) -> usize {
        self.line_to_first_row.get(line).copied().unwrap_or(0)
    }

    pub fn row_to_model_line(&self, row: usize) -> usize {
        self.segment_at(row).map(|s| s.model_line).unwrap_or(0)
    }

    pub fn total_rows(&self) -> usize {
        self.segments.len()
    }

    pub fn segments(&self) -> &[WrapSegment] {
        &self.segments
    }
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

        WrapSnapshot {
            segments,
            line_to_first_row,
        }
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

/// A span's visible byte length: `Identical`'s is its `range`'s length (no
/// `content` needed); `Substituted`'s is its own `text`'s length, which
/// differs from `range`'s length once wrap-sliced (module docs).
fn span_visible_len(span: &SyntaxSpan) -> usize {
    match span {
        SyntaxSpan::Identical { range, .. } => range.end - range.start,
        SyntaxSpan::Substituted { text, .. } => text.len(),
    }
}

/// Slice the original spans down to `[seg_start, seg_end)` of the
/// concatenated line text — port of `wrap_map.go`'s `sliceOriginalSpans`.
/// Visible text is sliced the same way for both variants; only the
/// buffer-range metadata differs (module docs): `Identical` re-bases
/// `range` to match (its text IS its buffer range); `Substituted` leaves
/// `range` at the span's full original value and rune-slices `cell_map` to
/// match instead (its text is NOT its buffer range — delimiters dropped).
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
    use crate::element::doc::DocMachine;
    use rune_core::buffer::Buffer;
    use rune_core::cursor::CursorSet;

    fn wrap_lines(content: &str, cursor_offset: usize, focused: bool, width: u16) -> WrapSnapshot {
        let buf = Buffer::new(content);
        let mut doc = DocMachine::new();
        doc.set_focus(focused);
        doc.sync_content(&buf);
        let cursors = CursorSet::new(cursor_offset.min(buf.len()));
        doc.sync_cursors(&buf, &cursors);
        let (lines, _snap) = crate::emit::emit(buf.content(), doc.blocks());
        WrapMap::new(width).sync(buf.content(), &lines)
    }

    #[test]
    fn short_line_is_a_single_segment() {
        let wrap = wrap_lines("hello world\n", 0, true, 80);
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
        let wrap = wrap_lines(content, 0, true, 11);
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
        // delimiters are dropped.
        let content = "x `aaaaaaaaaaaaaaaaaaaa` y\n";
        let wrap = wrap_lines(content, content.len(), true, 6);

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
