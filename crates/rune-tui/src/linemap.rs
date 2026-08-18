//! `LineMap`: the chokepoint translating between BUFFER coordinates and the
//! coordinates of a prefix-free text reconstructed from a set of physical
//! content lines.
//!
//! A fenced code block nested inside a blockquote or a list item does not
//! occupy one contiguous slice of the buffer: between two consecutive
//! content lines sits that container's own repeating prefix (`"> "`, a list
//! marker's indent). Those prefix bytes must never reach a parser as source
//! bytes — tree-sitter's error recovery silently absorbs a stray `"> "` for
//! some grammars but an indentation-sensitive one loses most of its
//! structure to it — so the text handed to a parser is rebuilt by joining
//! each line's own slice with a single `'\n'`. That joining `'\n'` IS the
//! buffer's real line terminator (the first byte of the gap), never a
//! prefix byte; every remaining gap byte has no counterpart in the
//! reconstructed text at all.
//!
//! Both translations are `O(log L)` in the number of lines: a prefix-sum
//! vector built once at construction turns each lookup into a
//! `partition_point`. The linear scan this replaced cost `O(S·L)` for `S`
//! spans, which a large fence made pathological — and the render path,
//! which runs per frame, cannot afford linear at all.

use std::ops::Range;

use rune_syntax::element::LineLocal;

/// A byte offset into the BUFFER's own content — the coordinate space
/// `LineMap`'s `lines` ranges are drawn from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct BufOffset(pub usize);

/// A byte offset into the `'\n'`-joined, container-prefix-free text
/// [`LineMap::reconstruct`] builds — a parser's own coordinate space, never
/// interchangeable with [`BufOffset`] without going through this module's
/// conversions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReconOffset(pub usize);

/// Which end of a reconstructed range [`LineMap::reconstructed_offset`] is
/// resolving — replaces a bare `is_end: bool` so the inclusive-end
/// convention it encodes reads at every call site instead of a naked
/// `true`/`false`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Endpoint {
    Start,
    End,
}

/// A fence's physical content lines plus the prefix sums that place each
/// line in the reconstructed text.
///
/// `prefix` has one more entry than `lines`: `prefix[i]` is where line `i`
/// begins in the reconstructed text and the final entry is that text's total
/// length. Every line but the last contributes its own content PLUS the
/// single joining `'\n'` that follows it; the last contributes only its
/// content, since nothing is joined after it.
#[derive(Clone, Debug, Default)]
pub struct LineMap {
    lines: Vec<Range<usize>>,
    /// The real BUFFER terminator width immediately after each line — `1`
    /// for a bare `'\n'`, `2` for a trimmed `"\r\n"` — meaningless for the
    /// last line, which has none. A CRLF line's own range is trimmed of its
    /// trailing `\r` below, so this is what lets [`LineMap::line_bounds`]
    /// still land exactly on the real `'\n'` instead of stopping one byte
    /// short, on the `\r` the trim just excluded.
    terminator: Vec<usize>,
    prefix: Vec<usize>,
}

impl LineMap {
    /// Builds the map from one fence's per-physical-line buffer ranges, in
    /// ascending buffer order (the order a document's own block parse
    /// produces them), against the same `content` those ranges index into.
    ///
    /// A CRLF line's raw range still carries its trailing `\r` — the buffer's
    /// own line index splits on `'\n'` alone — so each range is trimmed of
    /// exactly that byte here, before the prefix sums below are computed
    /// from it. Trimming the RANGE rather than the reconstructed text keeps
    /// the text and the offset bridge in agreement: every downstream sum is
    /// built from the same shortened lengths the text itself will slice to.
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

    /// The `'\n'`-joined, container-prefix-free text these lines reconstruct
    /// to, sliced out of `content`.
    ///
    /// `None` if any line fails to land on a live byte range of `content` —
    /// should not happen, since the ranges come from `content`'s own parse,
    /// but degrading to "skip this fence" rather than panicking keeps a
    /// rendering-layer bug from taking down the session. A partial
    /// reconstruction would be worse than none: it
    /// would silently shift every offset this map then translates.
    pub fn reconstruct(&self, content: &str) -> Option<String> {
        let pieces: Option<Vec<&str>> = self
            .lines
            .iter()
            .map(|line| content.get(line.clone()))
            .collect();
        Some(pieces?.join("\n"))
    }

    /// Maps a range of the reconstructed text back to the buffer offsets
    /// those bytes actually occupy — one [`LineLocal`] per physical line the
    /// range touches, each clipped to that line's own extent (its content
    /// plus, for every line but the last, its own real terminator).
    ///
    /// The mapping is piecewise-constant: within one line's own reconstructed
    /// span the shift from reconstructed to buffer offset is fixed, and
    /// changes only on crossing into the next line. A range spanning several
    /// lines is split at every such crossing BEFORE resolving offsets, so
    /// the pieces this returns are never joined into one contiguous span —
    /// doing that would silently include whatever container prefix sits in
    /// the buffer gap between two non-contiguous lines.
    ///
    /// Empty when `r` is inverted or empty, when either endpoint falls
    /// outside every line, or when a piece ends up empty — none of which
    /// should happen given ranges from this map's own reconstructed text,
    /// but degrading to "contributes nothing" here keeps a caller from ever
    /// painting a gap byte.
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

    /// Maps a range of the buffer to the reconstructed text's coordinates —
    /// the inverse of [`LineMap::to_buffer`], and its exact inverse: for
    /// every in-range reconstructed `r`, `to_reconstructed(to_buffer(r))`
    /// yields `r` back.
    ///
    /// A buffer offset landing in a GAP between two lines has no counterpart
    /// in the reconstructed text and yields `None` for the whole range,
    /// never a neighbouring line's offset. The single exception is a
    /// non-final line's `end` offset: that byte is the buffer's real line
    /// terminator, which the reconstructed text does carry as its joining
    /// `'\n'`, so it maps to that slot. The remaining gap bytes — the
    /// container prefix opening the next line — map nowhere. A FINAL line's
    /// `end` has no such slot and yields `None`.
    ///
    /// The end is resolved through the same inclusive-byte trick
    /// [`LineMap::to_buffer`] uses, so a range ending on a line boundary
    /// stays inside that line rather than probing the prefix that follows.
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

    /// The smallest reconstructed range covering every line that intersects
    /// the buffer range `r` — a deliberately CONSERVATIVE superset of
    /// [`LineMap::to_reconstructed`], defined for a range whose endpoints
    /// fall in a gap, before the first line, or past the last one.
    ///
    /// This is what a per-frame query wants and `to_reconstructed` is not:
    /// a viewport window is an arbitrary slice of the buffer that rarely
    /// lands on this region's own line boundaries, and an exact translation
    /// that reports `None` for such a window would leave the region
    /// unpainted. Widening to whole lines instead costs only a slightly
    /// larger query range, which a byte-range query treats as an
    /// intersection filter anyway.
    ///
    /// `None` when no line intersects `r` at all — including an empty or
    /// inverted `r`, and a map with no lines.
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

    /// The index of the line owning reconstructed `offset` — one past the
    /// count of prefix entries at or below it, since `prefix` is
    /// non-decreasing and starts at 0. `None` once `offset` reaches or
    /// passes the reconstructed length, where the count runs past the last
    /// line.
    fn line_owning(&self, offset: usize) -> Option<usize> {
        self.prefix.partition_point(|&p| p <= offset).checked_sub(1)
    }

    /// Line `i`'s own extent in BUFFER coordinates: its content, plus —
    /// for every line but the last — its own real terminator (`self.
    /// terminator[i]` bytes: the `'\n'` alone, or the `\r` this map trimmed
    /// out of the line plus the `'\n'` that follows it), which is the
    /// buffer's true line break and the reconstructed text's joining `'\n'`,
    /// never a container prefix byte.
    fn line_bounds(&self, i: usize) -> Option<Range<usize>> {
        let line = self.lines.get(i)?;
        let end = if i + 1 == self.lines.len() {
            line.end
        } else {
            line.end + self.terminator.get(i)?
        };
        Some(line.start..end)
    }

    /// The buffer offset line `i`'s own reconstructed within-line offset
    /// `within` maps to — [`LineMap::to_buffer`]'s single-offset chokepoint.
    ///
    /// A straight shift (`line.start + within`) is exactly right up to and
    /// including the joining `'\n'` itself (`within == len`, `len` this
    /// line's trimmed content length) and one past it (`within == len + 1`)
    /// — UNLESS this line's terminator is a trimmed `"\r\n"`: the `\r` sits
    /// between the content and the real `'\n'`, so a piece reaching past
    /// the content here would have to straddle a byte with no reconstructed
    /// counterpart of its own, which no single contiguous [`Range`] can do.
    /// Capping at the content's own end trades that one lost byte (the
    /// terminator paints nothing regardless) for never letting the `\r`
    /// back in — the whole reason it was trimmed in the first place.
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

    /// [`LineMap::to_reconstructed`]'s single-offset chokepoint.
    /// `Endpoint::End` requests the inclusive-byte convention: after
    /// locating the line owning `offset`, add one back so the result is an
    /// exclusive reconstructed offset again. `None` when `offset` falls in
    /// a gap — a container prefix byte, or anywhere before the first line
    /// or at or past the last line's end.
    fn reconstructed_offset(&self, offset: usize, endpoint: Endpoint) -> Option<usize> {
        // Lines are in ascending buffer order and never overlap, so the count
        // of starts at or below `offset` is one past the index of the only
        // line that could own it. An `offset` before the first line's start
        // counts nothing and fails the `checked_sub`.
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

/// A CRLF line's own byte range includes its trailing `\r` — the buffer's
/// line index splits on `'\n'` alone — so this drops that one byte before
/// [`LineMap::new`] ever sums a length from the range, and reports whether
/// it did so the caller can still find the real terminator past it.
/// `content` is peeked rather than trusted blindly: an out-of-range `line`
/// (should not happen, but the ranges arrive from a caller's own parse)
/// leaves the range untouched, and [`LineMap::reconstruct`] already
/// degrades a range that fails to land on `content` to "skip this fence"
/// rather than panicking.
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
mod tests {
    use super::*;

    fn recon(r: Range<usize>) -> Range<ReconOffset> {
        ReconOffset(r.start)..ReconOffset(r.end)
    }

    fn buf(r: Range<usize>) -> Range<BufOffset> {
        BufOffset(r.start)..BufOffset(r.end)
    }

    /// A top-level fence: consecutive lines are truly adjacent in the buffer
    /// (the gap is exactly one real `'\n'`), so the reconstructed text is
    /// byte-identical to the buffer slice.
    fn contiguous() -> (&'static str, LineMap) {
        let content = "let a = 1;\nlet b = 2;";
        (content, LineMap::new(content, vec![0..10, 11..21]))
    }

    /// A blockquoted fence: line 1 starts after `"> "`, two bytes the
    /// reconstructed text never sees.
    fn nested() -> (&'static str, LineMap) {
        let content = "let a = 1;\n> let b = 2;";
        (content, LineMap::new(content, vec![0..10, 13..23]))
    }

    /// A one-line fence against `content`. Written as a call rather than
    /// `vec![line]` because a single range literal inside a `vec!` is a
    /// hard clippy error.
    fn one_line(content: &str, line: Range<usize>) -> LineMap {
        LineMap::new(content, vec![line])
    }

    #[test]
    fn reconstruct_drops_a_container_prefix_but_keeps_a_contiguous_slice_verbatim() {
        let (content, map) = contiguous();
        assert_eq!(map.reconstruct(content).unwrap(), content);

        let (content, map) = nested();
        assert_eq!(map.reconstruct(content).unwrap(), "let a = 1;\nlet b = 2;");
    }

    #[test]
    fn reconstruct_reports_none_for_a_line_off_the_live_buffer() {
        let map = LineMap::new("short", vec![0..10, 40..50]);
        assert!(map.reconstruct("short").is_none());
    }

    #[test]
    fn to_buffer_is_identity_for_buffer_contiguous_lines() {
        let (content, map) = contiguous();
        assert_eq!(&content[11..21], "let b = 2;");

        let mapped = map.to_buffer(recon(11..15));
        assert_eq!(mapped.len(), 1, "a single-line range must map to one piece");
        assert_eq!(mapped[0].line(), 1);
        assert_eq!(mapped[0].range(), 11..15);
        assert_eq!(&content[mapped[0].range()], "let ");
    }

    #[test]
    fn to_buffer_skips_the_gap_between_nested_lines() {
        let (content, map) = nested();
        assert_eq!(&content[13..23], "let b = 2;");

        // Reconstructed text is "let a = 1;\nlet b = 2;", so offsets 11..14
        // are line 1's own "let" and must land on line 1's real buffer bytes,
        // never inside the "> " gap.
        let mapped = map.to_buffer(recon(11..14));
        assert_eq!(mapped.len(), 1);
        assert_eq!(&content[mapped[0].range()], "let");
    }

    #[test]
    fn to_buffer_end_boundary_never_lands_in_the_prefix() {
        let content = "ab\n> cd";
        let map = LineMap::new(content, vec![0..2, 5..7]);

        // Reconstructed text is "ab\ncd". A range covering "ab" plus its
        // joining '\n' must map to the real newline's own end in the buffer,
        // never into the "> " gap that follows it.
        let mapped = map.to_buffer(recon(0..3));
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].range(), 0..3);
        assert_eq!(&content[mapped[0].range()], "ab\n");
    }

    /// The load-bearing regression case: a token starting on one physical
    /// line and ending on the next must come back as TWO pieces, each
    /// entirely inside its own line's bounds — never a single contiguous
    /// range that would swallow the container prefix sitting in the gap
    /// between them.
    #[test]
    fn to_buffer_splits_a_cross_line_range_at_the_line_boundary_instead_of_spanning_the_gap() {
        let content = "let a = 1;\n> let b = 2;";
        let map = LineMap::new(content, vec![0..10, 13..23]);

        // Reconstructed text is "let a = 1;\nlet b = 2;". 8..14 covers
        // "1;\nlet" — the tail of line 0, the joining newline, and the head
        // of line 1.
        let mapped = map.to_buffer(recon(8..14));
        assert_eq!(
            mapped.len(),
            2,
            "a cross-line range must split into two pieces"
        );

        assert_eq!(mapped[0].line(), 0);
        assert_eq!(mapped[0].range(), 8..11);
        assert_eq!(&content[mapped[0].range()], "1;\n");

        assert_eq!(mapped[1].line(), 1);
        assert_eq!(mapped[1].range(), 13..16);
        assert_eq!(&content[mapped[1].range()], "let");

        let combined: String = mapped.iter().map(|p| &content[p.range()]).collect();
        assert_eq!(combined, "1;\nlet");
        assert!(
            !combined.contains("> "),
            "the pieces must never include the blockquote's own gap bytes"
        );
    }

    /// The CRLF defect at its own layer: a physical line's raw range still
    /// carries its trailing `\r`, and no piece `to_buffer` returns may ever
    /// include it — checked here directly against `LineMap`, one level below the
    /// end-to-end highlight pipeline the integration gate checks.
    #[test]
    fn to_buffer_never_lets_a_trimmed_carriage_return_back_into_a_piece() {
        let content = "one\r\ntwo\r\n";
        let map = LineMap::new(content, vec![0..4, 5..9]);
        assert_eq!(map.reconstruct(content).unwrap(), "one\ntwo");

        let mapped = map.to_buffer(recon(0..7));
        assert_eq!(mapped.len(), 2);
        for piece in &mapped {
            let text = &content[piece.range()];
            assert!(!text.contains('\r'), "piece text {text:?} carries a \\r");
        }
        let combined: String = mapped.iter().map(|p| &content[p.range()]).collect();
        assert_eq!(combined, "onetwo");
    }

    #[test]
    fn to_reconstructed_maps_a_non_final_line_end_to_the_joining_newline() {
        let (_, map) = nested();
        // Line 0 ends at buffer offset 10, which is the buffer's real '\n'
        // and the reconstructed text's joining '\n' at offset 10.
        assert_eq!(map.to_reconstructed(buf(10..11)), Some(recon(10..11)));
    }

    #[test]
    fn to_reconstructed_rejects_container_prefix_bytes() {
        let (_, map) = nested();
        // Buffer 11..13 is "> ", pure gap: no reconstructed counterpart, and
        // certainly not line 1's opening offset.
        assert_eq!(map.to_reconstructed(buf(11..12)), None);
        assert_eq!(map.to_reconstructed(buf(12..13)), None);
        // The byte right after the last line's end is past the text.
        assert_eq!(map.to_reconstructed(buf(23..24)), None);
    }

    #[test]
    fn to_reconstructed_rejects_offsets_before_the_first_line() {
        let map = one_line("01234567", 5..8);
        assert_eq!(map.to_reconstructed(buf(0..1)), None);
        assert_eq!(map.to_reconstructed(buf(4..5)), None);
        assert_eq!(map.to_reconstructed(buf(5..6)), Some(recon(0..1)));
    }

    #[test]
    fn empty_and_inverted_ranges_map_nowhere() {
        let (_, map) = contiguous();
        // Spelled as struct literals: a bare `3..3`/`5..2` is a hard clippy
        // error, and these degenerate inputs are exactly what is under test.
        let empty = Range { start: 3, end: 3 };
        let inverted = Range { start: 5, end: 2 };
        assert!(map.to_buffer(recon(empty.clone())).is_empty());
        assert!(map.to_buffer(recon(inverted.clone())).is_empty());
        assert_eq!(map.to_reconstructed(buf(empty)), None);
        assert_eq!(map.to_reconstructed(buf(inverted)), None);
    }

    #[test]
    fn an_empty_line_map_maps_nothing() {
        let map = LineMap::new("", vec![]);
        assert_eq!(map.reconstruct("anything").unwrap(), "");
        assert!(map.to_buffer(recon(0..1)).is_empty());
        assert_eq!(map.to_reconstructed(buf(0..1)), None);
    }

    #[test]
    fn a_single_line_maps_its_own_content_and_nothing_past_it() {
        let content = "abc";
        let map = one_line(content, 0..3);
        assert_eq!(map.reconstruct(content).unwrap(), "abc");
        let mapped = map.to_buffer(recon(0..3));
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].range(), 0..3);
        // A single line is also the LAST line, so it has no joining '\n':
        // offset 3 is past the reconstructed text in both directions.
        assert!(map.to_buffer(recon(3..4)).is_empty());
        assert_eq!(map.to_reconstructed(buf(3..4)), None);
    }

    #[test]
    fn a_blank_line_in_the_middle_still_carries_its_joining_newline() {
        // "a\n\nb": line 1 is empty and contributes only the '\n' that joins
        // it to line 2.
        let content = "a\n\nb";
        let map = LineMap::new(content, vec![0..1, 2..2, 3..4]);
        assert_eq!(map.reconstruct(content).unwrap(), "a\n\nb");

        // The blank line's own newline sits at buffer offset 2 and
        // reconstructed offset 2.
        let mapped = map.to_buffer(recon(2..3));
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].range(), 2..3);
        assert_eq!(map.to_reconstructed(buf(2..3)), Some(recon(2..3)));
    }

    /// The property the two directions exist to guarantee: every in-range
    /// reconstructed range survives a round trip through buffer coordinates
    /// unchanged — piece by piece, since a range crossing a line boundary
    /// now comes back as several. Run over both shapes a fence can take —
    /// buffer-contiguous and container-nested — since only the nested one
    /// exercises the gaps.
    #[test]
    fn every_reconstructed_range_round_trips_through_buffer_coordinates() {
        for (content, map) in [contiguous(), nested()] {
            let text = map.reconstruct(content).unwrap();
            for start in 0..text.len() {
                for end in (start + 1)..=text.len() {
                    let r = start..end;
                    let pieces = map.to_buffer(recon(r.clone()));
                    assert!(
                        !pieces.is_empty(),
                        "every in-range reconstructed range maps to the buffer"
                    );
                    let mut cursor = r.start;
                    for piece in &pieces {
                        let back = map
                            .to_reconstructed(buf(piece.range()))
                            .expect("a mapped piece must map back");
                        assert_eq!(back.start.0, cursor, "pieces must cover {r:?} contiguously");
                        cursor = back.end.0;
                    }
                    assert_eq!(cursor, r.end, "pieces must cover the whole of {r:?}");
                }
            }
        }
    }

    /// The render path's window translation: an arbitrary viewport slice
    /// widens to whole lines rather than reporting `None` the way the exact
    /// `to_reconstructed` does for an endpoint sitting in a container
    /// prefix.
    #[test]
    fn reconstructed_window_widens_a_gap_landing_window_to_whole_lines() {
        let (_, map) = nested();
        // Buffer 9..15 starts inside line 0 and ends inside line 1, crossing
        // the "> " prefix between them. `to_reconstructed` refuses the
        // prefix bytes outright; the window widens to both whole lines.
        assert_eq!(map.to_reconstructed(buf(11..12)), None);
        assert_eq!(map.reconstructed_window(buf(9..15)), Some(recon(0..21)));
    }

    /// A window falling ENTIRELY inside a container prefix intersects no
    /// line and therefore covers nothing — widening never invents bytes.
    #[test]
    fn reconstructed_window_reports_none_for_a_window_wholly_inside_a_gap() {
        let (_, map) = nested();
        assert_eq!(map.reconstructed_window(buf(11..13)), None);
    }

    #[test]
    fn reconstructed_window_covers_the_whole_text_for_an_oversized_window() {
        let (content, map) = nested();
        let len = map.reconstruct(content).unwrap().len();
        assert_eq!(map.reconstructed_window(buf(0..1000)), Some(recon(0..len)));
    }

    #[test]
    fn reconstructed_window_reports_none_when_no_line_intersects() {
        let map = one_line("01234567", 5..8);
        assert_eq!(map.reconstructed_window(buf(0..5)), None);
        assert_eq!(map.reconstructed_window(buf(9..20)), None);
        let empty = Range { start: 6, end: 6 };
        assert_eq!(map.reconstructed_window(buf(empty)), None);
        assert_eq!(
            LineMap::new("", vec![]).reconstructed_window(buf(0..10)),
            None
        );
    }

    /// A window covering only the second line must not drag the first
    /// line's bytes in with it — the widening is to whole INTERSECTING
    /// lines, never to the region's whole extent.
    #[test]
    fn reconstructed_window_starts_at_the_first_intersecting_line() {
        let (_, map) = nested();
        assert_eq!(map.reconstructed_window(buf(15..18)), Some(recon(11..21)));
    }

    #[test]
    fn an_out_of_range_reconstructed_offset_maps_nowhere() {
        let (content, map) = nested();
        let len = map.reconstruct(content).unwrap().len();
        assert!(map.to_buffer(recon(len..len + 1)).is_empty());
        assert!(map.to_buffer(recon(len + 100..len + 101)).is_empty());
    }
}
