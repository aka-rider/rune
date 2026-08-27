use std::ops::Range;

use rune_syntax::element::LineLocal;

// A byte offset into the BUFFER's own content — the coordinate space
// `LineMap`'s `lines` ranges are drawn from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct BufOffset(pub usize);

// A byte offset into the `'\n'`-joined, container-prefix-free text
// [`LineMap::reconstruct`] builds — a parser's own coordinate space, never
// interchangeable with [`BufOffset`] without going through this module's
// conversions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReconOffset(pub usize);

// Which end of a reconstructed range [`LineMap::reconstructed_offset`] is
// resolving — replaces a bare `is_end: bool` so the inclusive-end
// convention it encodes reads at every call site instead of a naked
// `true`/`false`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Endpoint {
    Start,
    End,
}

// The chokepoint translating between BUFFER coordinates and the
// coordinates of a prefix-free text reconstructed from a set of physical
// content lines: a fenced code block nested inside a blockquote or a list
// item does not occupy one contiguous buffer slice, so [`LineMap::
// reconstruct`] rebuilds it by joining each line's own slice with a single
// `'\n'`. Both translations are `O(log L)` in the line count via the
// prefix-sum vector below, replacing an `O(S·L)` linear scan a large fence
// made pathological on a path that runs per render frame.
//
// `prefix` has one more entry than `lines`: `prefix[i]` is where line `i`
// begins in the reconstructed text and the final entry is that text's total
// length. Every line but the last contributes its own content PLUS the
// single joining `'\n'` that follows it; the last contributes only its
// content, since nothing is joined after it.
#[derive(Clone, Debug, Default)]
pub struct LineMap {
    lines: Vec<Range<usize>>,
    // The real BUFFER terminator width immediately after each line — `1`
    // for a bare `'\n'`, `2` for a trimmed `"\r\n"` — meaningless for the
    // last line, which has none. A CRLF line's own range is trimmed of its
    // trailing `\r` below, so this is what lets [`LineMap::line_bounds`]
    // still land exactly on the real `'\n'` instead of stopping one byte
    // short, on the `\r` the trim just excluded.
    terminator: Vec<usize>,
    prefix: Vec<usize>,
}

impl LineMap {
    // Builds the map from one fence's per-physical-line buffer ranges, in
    // ascending buffer order (the order a document's own block parse
    // produces them), against the same `content` those ranges index into.
    //
    // A CRLF line's raw range still carries its trailing `\r` — the buffer's
    // own line index splits on `'\n'` alone — so each range is trimmed of
    // exactly that byte here, before the prefix sums below are computed
    // from it. Trimming the RANGE rather than the reconstructed text keeps
    // the text and the offset bridge in agreement: every downstream sum is
    // built from the same shortened lengths the text itself will slice to.
    pub fn new(content: &str, lines: Vec<Range<usize>>) -> LineMap {
        let mut trimmed = Vec::with_capacity(lines.len());
        let mut terminator = Vec::with_capacity(lines.len());
        for line in lines {
            let (line, had_cr) = trim_trailing_cr(content, line);
            trimmed.push(line);
            terminator.push(if had_cr { 2 } else { 1 });
        }
        let last = trimmed.len().saturating_sub(1);
        let mut prefix = Vec::with_capacity(trimmed.len() + 1);
        let mut cursor = 0usize;
        prefix.push(cursor);
        for (i, line) in trimmed.iter().enumerate() {
            let len = line.end.saturating_sub(line.start);
            cursor += if i == last { len } else { len + 1 };
            prefix.push(cursor);
        }
        LineMap {
            lines: trimmed,
            terminator,
            prefix,
        }
    }

    // The `'\n'`-joined, container-prefix-free text these lines reconstruct
    // to, sliced out of `content` — what a parser actually sees. A
    // container's own repeating prefix (`"> "`, a list marker's indent)
    // must never reach it as source bytes: tree-sitter's error recovery
    // silently absorbs a stray one for some grammars, but an
    // indentation-sensitive grammar loses most of its structure to it.
    //
    // `None` if any line fails to land on a live byte range of `content` —
    // degrading to "skip this fence" rather than panicking. A PARTIAL
    // reconstruction would be worse than none: it would silently shift
    // every offset this map then translates.
    pub fn reconstruct(&self, content: &str) -> Option<String> {
        let pieces: Option<Vec<&str>> = self
            .lines
            .iter()
            .map(|line| content.get(line.clone()))
            .collect();
        Some(pieces?.join("\n"))
    }

    // One [`LineLocal`] per physical line `r` touches, split at every line
    // crossing so no piece spans the buffer gap between two non-contiguous
    // lines.
    pub fn to_buffer(&self, r: Range<ReconOffset>) -> Vec<LineLocal> {
        let r = r.start.0..r.end.0;
        if r.start >= r.end {
            return Vec::new();
        }
        let (Some(first), Some(last)) = (self.line_owning(r.start), self.line_owning(r.end - 1))
        else {
            return Vec::new();
        };
        let mut pieces = Vec::with_capacity(last.saturating_sub(first) + 1);
        for i in first..=last {
            let Some(&line_prefix) = self.prefix.get(i) else {
                break;
            };
            let local_start = if i == first { r.start } else { line_prefix };
            let local_end = if i == last {
                r.end
            } else {
                self.prefix.get(i + 1).copied().unwrap_or(local_start)
            };
            if local_start >= local_end {
                continue;
            }
            let (Some(mapped_start), Some(mapped_end)) = (
                self.line_offset_to_buffer(i, local_start - line_prefix),
                self.line_offset_to_buffer(i, local_end - line_prefix),
            ) else {
                continue;
            };
            let Some(bounds) = self.line_bounds(i) else {
                continue;
            };
            if let Some(piece) = LineLocal::clip(i, bounds, mapped_start..mapped_end) {
                pieces.push(piece);
            }
        }
        pieces
    }

    // The exact inverse of [`LineMap::to_buffer`]. A buffer offset in a gap
    // between two lines has no counterpart and yields `None` for the whole
    // range — the one exception is a non-final line's own `end` offset,
    // which the reconstructed text carries as its joining `'\n'`.
    pub fn to_reconstructed(&self, r: Range<BufOffset>) -> Option<Range<ReconOffset>> {
        let r = r.start.0..r.end.0;
        if r.start >= r.end {
            return None;
        }
        let start = self.reconstructed_offset(r.start, Endpoint::Start)?;
        let end = self.reconstructed_offset(r.end - 1, Endpoint::End)?;
        if start >= end {
            return None;
        }
        Some(ReconOffset(start)..ReconOffset(end))
    }

    // The smallest reconstructed range covering every line that intersects
    // the buffer range `r` — a deliberately conservative superset of
    // [`LineMap::to_reconstructed`], for a caller (a per-frame viewport
    // query) whose window rarely lands on a line boundary and needs an
    // answer anyway rather than leaving the region unpainted.
    pub fn reconstructed_window(&self, r: Range<BufOffset>) -> Option<Range<ReconOffset>> {
        let r = r.start.0..r.end.0;
        if r.start >= r.end {
            return None;
        }
        // Lines are in ascending buffer order and never overlap, so both
        // ends are non-decreasing and each boundary is a `partition_point`.
        // A line is skipped only when it ends strictly before `r` begins or
        // begins at or after `r` ends.
        let first = self.lines.partition_point(|line| line.end < r.start);
        let last = self.lines.partition_point(|line| line.start < r.end);
        let last_index = last.checked_sub(1)?;
        if first > last_index {
            return None;
        }
        let start = *self.prefix.get(first)?;
        let line = self.lines.get(last_index)?;
        let end = self.prefix.get(last_index)? + line.end.saturating_sub(line.start);
        if start >= end {
            None
        } else {
            Some(ReconOffset(start)..ReconOffset(end))
        }
    }

    // The index of the line owning reconstructed `offset` — one past the
    // count of prefix entries at or below it, since `prefix` is
    // non-decreasing and starts at 0. `None` once `offset` reaches or
    // passes the reconstructed length, where the count runs past the last
    // line.
    fn line_owning(&self, offset: usize) -> Option<usize> {
        self.prefix.partition_point(|&p| p <= offset).checked_sub(1)
    }

    fn line_bounds(&self, i: usize) -> Option<Range<usize>> {
        let line = self.lines.get(i)?;
        let end = if i + 1 == self.lines.len() {
            line.end
        } else {
            line.end + self.terminator.get(i)?
        };
        Some(line.start..end)
    }

    // A straight shift (`line.start + within`) is exactly right, UNLESS
    // this line's terminator is a trimmed `"\r\n"`: the `\r` sits between
    // the content and the real `'\n'`, so a piece reaching past the content
    // would have to straddle a byte with no reconstructed counterpart,
    // which no single contiguous [`Range`] can do. Capping at the content's
    // own end trades that one unpaintable byte for never letting the `\r`
    // back in.
    fn line_offset_to_buffer(&self, i: usize, within: usize) -> Option<usize> {
        let line = self.lines.get(i)?;
        let len = line.end.saturating_sub(line.start);
        let crlf_terminator =
            i + 1 != self.lines.len() && self.terminator.get(i).copied().unwrap_or(1) == 2;
        if crlf_terminator && within > len {
            return Some(line.start + len);
        }
        Some(line.start + within)
    }

    // `Endpoint::End` requests the inclusive-byte convention: after
    // locating the line owning `offset`, add one back so the result is an
    // exclusive reconstructed offset again.
    fn reconstructed_offset(&self, offset: usize, endpoint: Endpoint) -> Option<usize> {
        let i = self
            .lines
            .partition_point(|line| line.start <= offset)
            .checked_sub(1)?;
        let line = self.lines.get(i)?;
        let owned_end = self.line_bounds(i)?.end;
        if offset >= owned_end {
            return None;
        }
        let mapped = self.prefix.get(i)? + (offset - line.start);
        Some(match endpoint {
            Endpoint::End => mapped + 1,
            Endpoint::Start => mapped,
        })
    }
}

// A CRLF line's own byte range includes its trailing `\r` — the buffer's
// line index splits on `'\n'` alone — so this drops that one byte before
// [`LineMap::new`] ever sums a length from the range, and reports whether
// it did so the caller can still find the real terminator past it.
fn trim_trailing_cr(content: &str, line: Range<usize>) -> (Range<usize>, bool) {
    let ends_in_cr = line.end > line.start
        && line
            .end
            .checked_sub(1)
            .and_then(|i| content.as_bytes().get(i))
            == Some(&b'\r');
    if ends_in_cr {
        (line.start..line.end - 1, true)
    } else {
        (line, false)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
#[path = "linemap_tests.rs"]
mod tests;
