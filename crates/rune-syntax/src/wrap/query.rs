//! The coordinate-query half of the wrap pass (CONSTITUTION §1.6 split of
//! the wrap module, plan Context "Emit -> wrap -> snapshot"): `WrapSnapshot`
//! answers buffer/syntax/wrap coordinate-conversion and visual-column
//! questions about the segments `super::WrapMap` already computed. It
//! stores no copy of the document — an `Identical` span's visible text is
//! recovered from the caller-supplied `content: &str` on every query that
//! needs it (`visual_col`/`byte_col_from_visual`), never cached here, so a
//! `WrapSnapshot` never owns an O(document) allocation of its own.

use super::WrapSegment;
use super::width::{grapheme_width_with_tab, next_grapheme};
use crate::syntax::SyntaxSpan;
use rune_core::coords::{SyntaxPoint, WrapPoint};

/// A span's visible byte length: `Identical`'s is its `range`'s length (no
/// `content` needed); `Substituted`'s is its own `text`'s length, which
/// differs from `range`'s length once wrap-sliced (see this module tree's
/// own docs). `pub(super)` because the table branch of the wrap pass needs
/// the same visible-length count to compute a Wrapped/Pivoted extra row's
/// `start_col` — one shared chokepoint rather than a second copy of this
/// match.
pub(super) fn span_visible_len(span: &SyntaxSpan) -> usize {
    match span {
        SyntaxSpan::Identical { range, .. } => range.end - range.start,
        SyntaxSpan::Substituted { text, .. } => text.len(),
    }
}

/// Concatenates every span's own visible text, alongside where each span
/// ENDS within that concatenation (ascending, last entry == the
/// concatenation's own length) — the shared building block behind the
/// free-function width walkers below, so both take a plain `&[SyntaxSpan]`
/// rather than needing a whole `WrapSegment` to key off (WP3: a
/// `DisplayRow`'s synthesised border spans have no backing `WrapSegment` at
/// all, only their own `spans`). The bounds feed straight into
/// `next_grapheme` so these walkers draw cluster boundaries
/// at the exact same byte positions the row's actual `Cell`s do — see that
/// function's docs for why a bare concatenated-string walk would disagree.
fn spans_text_and_bounds(content: &str, spans: &[SyntaxSpan]) -> (String, Vec<usize>) {
    let mut text = String::new();
    let mut bounds = Vec::with_capacity(spans.len());
    for sp in spans {
        text.push_str(sp.text(content));
        bounds.push(text.len());
    }
    (text, bounds)
}

/// The width-walking core of `WrapSnapshot::visual_col` — moved to a free
/// function over a plain `&[SyntaxSpan]` (WP3.S3) so any span slice can be
/// walked identically, not only one already indexed by wrap row.
pub(super) fn visual_col(content: &str, spans: &[SyntaxSpan], byte_col: usize) -> usize {
    let (text, bounds) = spans_text_and_bounds(content, spans);
    cells_up_to(&text, &bounds, byte_col)
}

/// The shared grapheme-walking core behind `visual_col` (row-relative,
/// walks a wrap row's concatenated span text) and `line_visual_col`
/// (line-relative, walks a whole logical line's raw text) — both count
/// terminal cells up to `byte_col` over the same `next_grapheme`/
/// `grapheme_width_with_tab` chokepoint, differing only in what `text`/
/// `bounds` they're handed, so the two callers can never disagree on how a
/// cluster is measured.
fn cells_up_to(text: &str, bounds: &[usize], byte_col: usize) -> usize {
    let mut visual = 0usize;
    let mut bytes = 0usize;
    while bytes < text.len() && bytes < byte_col {
        let Some(cluster) = next_grapheme(text, bounds, bytes) else {
            break;
        };
        visual += grapheme_width_with_tab(cluster, visual);
        bytes += cluster.len();
    }
    visual
}

/// The largest grapheme-cluster boundary in `spans`' concatenated visible
/// text at or before `byte_col` — the `wrap_to_syntax` counterpart of
/// `cells_up_to`'s width walk, over the exact same `next_grapheme` chokepoint
/// so the two can never disagree about where a cluster starts. Unlike
/// `byte_col_from_visual` (which structurally can't return mid-cluster,
/// since it only ever advances `bytes` by whole `cluster.len()`s),
/// `wrap_to_syntax`'s `wp.col` arrives as an already-computed byte offset
/// that a caller could hand in landing anywhere — this snaps it DOWN to the
/// nearest cluster start rather than trusting it.
fn snap_to_grapheme_boundary(content: &str, spans: &[SyntaxSpan], byte_col: usize) -> usize {
    let (text, bounds) = spans_text_and_bounds(content, spans);
    let mut bytes = 0usize;
    while bytes < text.len() {
        let Some(cluster) = next_grapheme(&text, &bounds, bytes) else {
            break;
        };
        let next_bytes = bytes + cluster.len();
        if next_bytes > byte_col {
            break;
        }
        bytes = next_bytes;
    }
    bytes
}

/// A LINE-relative cell column: the terminal-cell width of `line_text` up
/// to byte offset `byte_col`, measured over grapheme clusters through the
/// same chokepoint `visual_col`/`wrap_line` use. Distinct from `visual_col`
/// (which is WRAP-ROW-relative — it answers "how many cells into THIS
/// wrapped row", resetting at 0 for every row of a wrapped line): this
/// answers "how many cells into the whole logical line", the unit a
/// footer's Ln/Col readout needs, since a wrapped line's row 2+ must not
/// restart the column count. Callers with only a line's raw text (no
/// `WrapSnapshot`/`SyntaxSpan`s in hand) use this directly; callers already
/// holding a row's spans go through `visual_col`.
pub fn line_visual_col(line_text: &str, byte_col: usize) -> usize {
    let bounds = [line_text.len()];
    cells_up_to(line_text, &bounds, byte_col)
}

/// The width-walking core of `WrapSnapshot::byte_col_from_visual` — the
/// `visual_col` counterpart, same rationale (WP3.S3).
pub(super) fn byte_col_from_visual(
    content: &str,
    spans: &[SyntaxSpan],
    visual_col: usize,
) -> usize {
    let (text, bounds) = spans_text_and_bounds(content, spans);
    let mut visual = 0usize;
    let mut bytes = 0usize;
    while bytes < text.len() {
        let Some(cluster) = next_grapheme(&text, &bounds, bytes) else {
            break;
        };
        let rw = grapheme_width_with_tab(cluster, visual);
        if visual + rw > visual_col {
            break;
        }
        visual += rw;
        bytes += cluster.len();
    }
    bytes
}

#[derive(Clone, Debug, Default)]
pub struct WrapSnapshot {
    // `segments` IS the row index at THIS layer — row `i` is always
    // `segments[i]`, no separate `row_to_segment` indirection. Border-row
    // synthesis around a rendered table (`rune-md`'s `expand_tables`) is a
    // one-layer-up DISPLAY-space concern: it inserts synthetic rows
    // between wrap rows, so `segments` here stays exactly what the wrap
    // pass itself produced, one entry per real wrap row.
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

    /// `content` is needed to snap `wp.col` down to a grapheme-cluster
    /// boundary ([rune-syntax 3]): clamping to `seg_len` alone (as
    /// `byte_col_from_visual`'s sibling used to) still lets a byte column
    /// land mid-codepoint inside a multi-byte cluster, since a byte length
    /// clamp knows nothing about where characters actually start. Every
    /// current caller already has the buffer content in hand for the same
    /// reason `byte_col_from_visual` takes it.
    pub fn wrap_to_syntax(&self, content: &str, wp: WrapPoint) -> SyntaxPoint {
        let Some(seg) = self.segment_at(wp.row) else {
            return SyntaxPoint { line: 0, col: 0 };
        };
        let seg_len = Self::segment_len(seg);
        let col = snap_to_grapheme_boundary(content, &seg.spans, wp.col.min(seg_len));
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
        visual_col(content, &seg.spans, byte_col)
    }

    pub fn byte_col_from_visual(&self, content: &str, row: usize, visual_col: usize) -> usize {
        let Some(seg) = self.segment_at(row) else {
            return 0;
        };
        byte_col_from_visual(content, &seg.spans, visual_col)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_visual_col_counts_ascii_bytes_as_cells() {
        assert_eq!(line_visual_col("hello", 0), 0);
        assert_eq!(line_visual_col("hello", 3), 3);
        assert_eq!(line_visual_col("hello", 5), 5);
    }

    #[test]
    fn line_visual_col_treats_an_nfd_cluster_as_one_cell() {
        // "café" as NFD: e + combining acute (U+0301) is one grapheme
        // cluster, one cell — a per-`char` walk would report 2.
        let nfd = "cafe\u{0301}";
        let accent_start = "cafe".len();
        let end = nfd.len();
        assert_eq!(line_visual_col(nfd, accent_start), 4);
        assert_eq!(line_visual_col(nfd, end), 4);
    }

    #[test]
    fn line_visual_col_counts_cjk_as_two_cells() {
        let s = "a\u{4e2d}\u{6587}b"; // a 中 文 b
        let after_a = "a".len();
        let after_first_cjk = "a\u{4e2d}".len();
        let after_second_cjk = "a\u{4e2d}\u{6587}".len();
        let end = s.len();
        assert_eq!(line_visual_col(s, after_a), 1);
        assert_eq!(line_visual_col(s, after_first_cjk), 3);
        assert_eq!(line_visual_col(s, after_second_cjk), 5);
        assert_eq!(line_visual_col(s, end), 6);
    }

    #[test]
    fn line_visual_col_expands_a_tab_to_the_next_stop() {
        // A tab at column 0 expands to 4; a second tab from column 4
        // expands to a full stop (4 more, since 4 % 4 == 0).
        let s = "\t\ta";
        let after_first_tab = "\t".len();
        let after_second_tab = "\t\t".len();
        let end = s.len();
        assert_eq!(line_visual_col(s, after_first_tab), 4);
        assert_eq!(line_visual_col(s, after_second_tab), 8);
        assert_eq!(line_visual_col(s, end), 9);
    }

    #[test]
    fn line_visual_col_is_line_relative_not_row_relative() {
        // The whole point of this helper vs `visual_col`: it never resets
        // at a wrap-row boundary because it never sees one — it only ever
        // sees one logical line's own text.
        let long_line = "x".repeat(50);
        assert_eq!(line_visual_col(&long_line, 50), 50);
    }
}
