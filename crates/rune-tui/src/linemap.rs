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
    prefix: Vec<usize>,
}

impl LineMap {
    /// Builds the map from one fence's per-physical-line buffer ranges, in
    /// ascending buffer order (the order a document's own block parse
    /// produces them).
    pub fn new(lines: Vec<Range<usize>>) -> LineMap {
        let last = lines.len().saturating_sub(1);
        let mut prefix = Vec::with_capacity(lines.len() + 1);
        let mut cursor = 0usize;
        prefix.push(cursor);
        for (i, line) in lines.iter().enumerate() {
            let len = line.end.saturating_sub(line.start);
            cursor += if i == last { len } else { len + 1 };
            prefix.push(cursor);
        }
        LineMap { lines, prefix }
    }

    /// The `'\n'`-joined, container-prefix-free text these lines reconstruct
    /// to, sliced out of `content`.
    ///
    /// `None` if any line fails to land on a live byte range of `content` —
    /// should not happen, since the ranges come from `content`'s own parse,
    /// but degrading to "skip this fence" rather than panicking is the §1.3
    /// convention. A partial reconstruction would be worse than none: it
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
    /// those bytes actually occupy.
    ///
    /// The mapping is piecewise-constant: within one line's own reconstructed
    /// span the shift from reconstructed to buffer offset is fixed, and
    /// changes only on crossing into the next line. The end is resolved
    /// through the LAST byte the range covers (`r.end - 1`) and shifted back
    /// by one afterwards, so an end landing exactly on a line boundary
    /// resolves to the position right after that line's own newline — never
    /// into the following line's excluded prefix bytes, which is where a
    /// naive lookup of `r.end` itself would wander.
    ///
    /// `None` on any inconsistency (an out-of-range offset, no lines, or an
    /// inverted or empty `r`) means "drop this range".
    pub fn to_buffer(&self, r: Range<usize>) -> Option<Range<usize>> {
        if r.start >= r.end {
            return None;
        }
        let start = self.buffer_offset(r.start, false)?;
        let end = self.buffer_offset(r.end - 1, true)?;
        if start >= end {
            return None;
        }
        Some(start..end)
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
    pub fn to_reconstructed(&self, r: Range<usize>) -> Option<Range<usize>> {
        if r.start >= r.end {
            return None;
        }
        let start = self.reconstructed_offset(r.start, false)?;
        let end = self.reconstructed_offset(r.end - 1, true)?;
        if start >= end {
            return None;
        }
        Some(start..end)
    }

    /// [`LineMap::to_buffer`]'s single-offset chokepoint. `is_end` requests
    /// the inclusive-byte convention: after locating the line owning
    /// `offset`, add one back so the result is an exclusive buffer offset
    /// again.
    fn buffer_offset(&self, offset: usize, is_end: bool) -> Option<usize> {
        // `prefix` is non-decreasing and starts at 0, so the count of entries
        // at or below `offset` is one past the index of the owning line. An
        // `offset` at or beyond the reconstructed length counts every entry
        // and indexes past `lines`, where the `get` below reports `None`.
        let i = self
            .prefix
            .partition_point(|&p| p <= offset)
            .checked_sub(1)?;
        let line = self.lines.get(i)?;
        let within = offset - self.prefix.get(i)?;
        let mapped = line.start + within;
        Some(if is_end { mapped + 1 } else { mapped })
    }

    /// [`LineMap::to_reconstructed`]'s single-offset chokepoint, mirroring
    /// `buffer_offset`'s `is_end` convention. `None` when `offset` falls in a
    /// gap — a container prefix byte, or anywhere before the first line or at
    /// or past the last line's end.
    fn reconstructed_offset(&self, offset: usize, is_end: bool) -> Option<usize> {
        // Lines are in ascending buffer order and never overlap, so the count
        // of starts at or below `offset` is one past the index of the only
        // line that could own it. An `offset` before the first line's start
        // counts nothing and fails the `checked_sub`.
        let i = self
            .lines
            .partition_point(|line| line.start <= offset)
            .checked_sub(1)?;
        let line = self.lines.get(i)?;
        // A non-final line owns one byte past its content: the buffer's real
        // newline, which the reconstructed text carries as its joining '\n'.
        let owned_end = if i + 1 == self.lines.len() {
            line.end
        } else {
            line.end + 1
        };
        if offset >= owned_end {
            return None;
        }
        let mapped = self.prefix.get(i)? + (offset - line.start);
        Some(if is_end { mapped + 1 } else { mapped })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// A top-level fence: consecutive lines are truly adjacent in the buffer
    /// (the gap is exactly one real `'\n'`), so the reconstructed text is
    /// byte-identical to the buffer slice.
    fn contiguous() -> (&'static str, LineMap) {
        let content = "let a = 1;\nlet b = 2;";
        (content, LineMap::new(vec![0..10, 11..21]))
    }

    /// A blockquoted fence: line 1 starts after `"> "`, two bytes the
    /// reconstructed text never sees.
    fn nested() -> (&'static str, LineMap) {
        let content = "let a = 1;\n> let b = 2;";
        (content, LineMap::new(vec![0..10, 13..23]))
    }

    /// A one-line fence. Written as a call rather than `vec![0..3]` because
    /// a single range literal inside a `vec!` is a hard clippy error.
    fn one_line(line: Range<usize>) -> LineMap {
        LineMap::new(vec![line])
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
        let map = LineMap::new(vec![0..10, 40..50]);
        assert!(map.reconstruct("short").is_none());
    }

    #[test]
    fn to_buffer_is_identity_for_buffer_contiguous_lines() {
        let (content, map) = contiguous();
        assert_eq!(&content[11..21], "let b = 2;");

        let mapped = map.to_buffer(11..15).expect("in range");
        assert_eq!(mapped, 11..15);
        assert_eq!(&content[mapped], "let ");
    }

    #[test]
    fn to_buffer_skips_the_gap_between_nested_lines() {
        let (content, map) = nested();
        assert_eq!(&content[13..23], "let b = 2;");

        // Reconstructed text is "let a = 1;\nlet b = 2;", so offsets 11..14
        // are line 1's own "let" and must land on line 1's real buffer bytes,
        // never inside the "> " gap.
        let mapped = map.to_buffer(11..14).expect("in range");
        assert_eq!(&content[mapped], "let");
    }

    #[test]
    fn to_buffer_end_boundary_never_lands_in_the_prefix() {
        let content = "ab\n> cd";
        let map = LineMap::new(vec![0..2, 5..7]);

        // Reconstructed text is "ab\ncd". A range covering "ab" plus its
        // joining '\n' must map to the real newline's own end in the buffer,
        // never into the "> " gap that follows it.
        let mapped = map.to_buffer(0..3).expect("in range");
        assert_eq!(mapped, 0..3);
        assert_eq!(&content[mapped], "ab\n");
    }

    #[test]
    fn to_reconstructed_maps_a_non_final_line_end_to_the_joining_newline() {
        let (_, map) = nested();
        // Line 0 ends at buffer offset 10, which is the buffer's real '\n'
        // and the reconstructed text's joining '\n' at offset 10.
        assert_eq!(map.to_reconstructed(10..11), Some(10..11));
    }

    #[test]
    fn to_reconstructed_rejects_container_prefix_bytes() {
        let (_, map) = nested();
        // Buffer 11..13 is "> ", pure gap: no reconstructed counterpart, and
        // certainly not line 1's opening offset.
        assert_eq!(map.to_reconstructed(11..12), None);
        assert_eq!(map.to_reconstructed(12..13), None);
        // The byte right after the last line's end is past the text.
        assert_eq!(map.to_reconstructed(23..24), None);
    }

    #[test]
    fn to_reconstructed_rejects_offsets_before_the_first_line() {
        let map = one_line(5..8);
        assert_eq!(map.to_reconstructed(0..1), None);
        assert_eq!(map.to_reconstructed(4..5), None);
        assert_eq!(map.to_reconstructed(5..6), Some(0..1));
    }

    #[test]
    fn empty_and_inverted_ranges_map_nowhere() {
        let (_, map) = contiguous();
        // Spelled as struct literals: a bare `3..3`/`5..2` is a hard clippy
        // error, and these degenerate inputs are exactly what is under test.
        let empty = Range { start: 3, end: 3 };
        let inverted = Range { start: 5, end: 2 };
        assert_eq!(map.to_buffer(empty.clone()), None);
        assert_eq!(map.to_buffer(inverted.clone()), None);
        assert_eq!(map.to_reconstructed(empty), None);
        assert_eq!(map.to_reconstructed(inverted), None);
    }

    #[test]
    fn an_empty_line_map_maps_nothing() {
        let map = LineMap::new(vec![]);
        assert_eq!(map.reconstruct("anything").unwrap(), "");
        assert_eq!(map.to_buffer(0..1), None);
        assert_eq!(map.to_reconstructed(0..1), None);
    }

    #[test]
    fn a_single_line_maps_its_own_content_and_nothing_past_it() {
        let content = "abc";
        let map = one_line(0..3);
        assert_eq!(map.reconstruct(content).unwrap(), "abc");
        assert_eq!(map.to_buffer(0..3), Some(0..3));
        // A single line is also the LAST line, so it has no joining '\n':
        // offset 3 is past the reconstructed text in both directions.
        assert_eq!(map.to_buffer(3..4), None);
        assert_eq!(map.to_reconstructed(3..4), None);
    }

    #[test]
    fn a_blank_line_in_the_middle_still_carries_its_joining_newline() {
        // "a\n\nb": line 1 is empty and contributes only the '\n' that joins
        // it to line 2.
        let content = "a\n\nb";
        let map = LineMap::new(vec![0..1, 2..2, 3..4]);
        assert_eq!(map.reconstruct(content).unwrap(), "a\n\nb");

        // The blank line's own newline sits at buffer offset 2 and
        // reconstructed offset 2.
        assert_eq!(map.to_buffer(2..3), Some(2..3));
        assert_eq!(map.to_reconstructed(2..3), Some(2..3));
    }

    /// The property the two directions exist to guarantee: every in-range
    /// reconstructed range survives a round trip through buffer coordinates
    /// unchanged. Run over both shapes a fence can take — buffer-contiguous
    /// and container-nested — since only the nested one exercises the gaps.
    #[test]
    fn every_reconstructed_range_round_trips_through_buffer_coordinates() {
        for (content, map) in [contiguous(), nested()] {
            let text = map.reconstruct(content).unwrap();
            for start in 0..text.len() {
                for end in (start + 1)..=text.len() {
                    let r = start..end;
                    let buffer = map
                        .to_buffer(r.clone())
                        .expect("every in-range reconstructed range maps to the buffer");
                    assert_eq!(map.to_reconstructed(buffer), Some(r));
                }
            }
        }
    }

    #[test]
    fn an_out_of_range_reconstructed_offset_maps_nowhere() {
        let (content, map) = nested();
        let len = map.reconstruct(content).unwrap().len();
        assert_eq!(map.to_buffer(len..len + 1), None);
        assert_eq!(map.to_buffer(len + 100..len + 101), None);
    }
}
