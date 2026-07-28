//! The coordinate-query half of the wrap pass (CONSTITUTION §1.6 split of
//! `wrap.rs`, plan Context "Emit -> wrap -> snapshot"): `WrapSnapshot`
//! answers buffer/syntax/wrap coordinate-conversion and visual-column
//! questions about the segments `super::WrapMap` already computed. It
//! stores no copy of the document — an `Identical` span's visible text is
//! recovered from the caller-supplied `content: &str` on every query that
//! needs it (`visual_col`/`byte_col_from_visual`), never cached here, so a
//! `WrapSnapshot` never owns an O(document) allocation of its own.

use super::{WrapSegment, rune_width_with_tab};
use crate::syntax::SyntaxSpan;
use rune_core::coords::{SyntaxPoint, WrapPoint};

/// A span's visible byte length: `Identical`'s is its `range`'s length (no
/// `content` needed); `Substituted`'s is its own `text`'s length, which
/// differs from `range`'s length once wrap-sliced (module docs in
/// `wrap/mod.rs`).
fn span_visible_len(span: &SyntaxSpan) -> usize {
    match span {
        SyntaxSpan::Identical { range, .. } => range.end - range.start,
        SyntaxSpan::Substituted { text, .. } => text.len(),
    }
}

#[derive(Clone, Debug, Default)]
pub struct WrapSnapshot {
    // Phase 1 has no table/image row expansion, so `segments` IS the row
    // index — row `i` is always `segments[i]` (no separate `row_to_segment`
    // indirection; Go's original keeps one because post-expansion the two
    // can diverge, but nothing here needs that yet — reintroduce it in
    // Phase 5 alongside the expansion pass that actually requires it).
    segments: Vec<WrapSegment>,
    line_to_first_row: Vec<usize>,
}

impl WrapSnapshot {
    pub(crate) fn new(segments: Vec<WrapSegment>, line_to_first_row: Vec<usize>) -> WrapSnapshot {
        WrapSnapshot {
            segments,
            line_to_first_row,
        }
    }

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
