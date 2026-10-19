use std::ops::Range;

use super::{Buffer, Edit};
use crate::coords::{BufferOffset, BufferPoint};
use crate::cursor::Cursor;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct LineStarts {
    subsequent: Vec<usize>,
}

impl LineStarts {
    pub(super) fn from_full(full: Vec<usize>) -> LineStarts {
        LineStarts {
            subsequent: full.into_iter().skip(1).collect(),
        }
    }

    fn to_full(&self) -> Vec<usize> {
        std::iter::once(0)
            .chain(self.subsequent.iter().copied())
            .collect()
    }

    pub(super) fn len(&self) -> usize {
        self.subsequent.len() + 1
    }

    pub(super) fn get(&self, n: usize) -> Option<usize> {
        match n {
            0 => Some(0),
            _ => self.subsequent.get(n - 1).copied(),
        }
    }

    fn line_at(&self, offset: usize) -> usize {
        line_index_of(&self.subsequent, offset)
    }
}

impl Buffer {
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    pub fn line_start(&self, n: usize) -> Option<usize> {
        self.line_starts.get(n)
    }

    pub fn line_end(&self, n: usize) -> Option<usize> {
        let count = self.line_starts.len();
        if n >= count {
            return None;
        }
        if n == count - 1 {
            return Some(self.content.len());
        }
        Some(self.line_starts.get(n + 1)?.saturating_sub(1))
    }

    pub fn line(&self, n: usize) -> &str {
        let (Some(start), Some(end)) = (self.line_start(n), self.line_end(n)) else {
            return "";
        };
        if start <= end && end <= self.content.len() {
            self.content.get(start..end).unwrap_or("")
        } else {
            ""
        }
    }

    pub fn offset_to_line_col(&self, offset: usize) -> BufferPoint {
        let offset = offset.min(self.content.len());
        let line = self.line_starts.line_at(offset);
        let line_start = self.line_starts.get(line).unwrap_or(0);
        BufferPoint {
            line,
            col: offset.saturating_sub(line_start),
        }
    }

    pub fn caret_offset(&self, bp: BufferPoint) -> usize {
        let (Some(start), Some(content_end)) = (
            self.line_starts.get(bp.line),
            self.line_content_end(bp.line),
        ) else {
            return self.content.len();
        };
        super::snap_char_boundary(&self.content, start.saturating_add(bp.col).min(content_end))
    }

    pub fn line_content_end(&self, n: usize) -> Option<usize> {
        let end = self.line_end(n)?;
        if n + 1 == self.line_starts.len() {
            return Some(end);
        }
        Some(self.strip_trailing_cr(end))
    }

    pub fn line_terminator_range(&self, n: usize) -> Option<Range<usize>> {
        let end = self.line_end(n)?;
        if n + 1 == self.line_starts.len() {
            return Some(end..end);
        }
        let content_end = self.strip_trailing_cr(end);
        Some(content_end..end + 1)
    }

    pub fn line_break_at(&self, offset: usize) -> &'static str {
        let line = self.offset_to_line_col(offset).line;
        (0..=line)
            .rev()
            .find_map(|n| match self.line_terminator_range(n) {
                Some(term) if term.len() == 2 => Some("\r\n"),
                Some(term) if term.len() == 1 => Some("\n"),
                _ => None,
            })
            .unwrap_or("\n")
    }

    fn strip_trailing_cr(&self, end: usize) -> usize {
        if end > 0 && self.content.as_bytes().get(end - 1) == Some(&b'\r') {
            end - 1
        } else {
            end
        }
    }

    pub fn crlf_pair_at(&self, offset: usize) -> Option<Range<usize>> {
        let bytes = self.content.as_bytes();
        if bytes.get(offset) == Some(&b'\r') && bytes.get(offset + 1) == Some(&b'\n') {
            return Some(offset..offset + 2);
        }
        if offset > 0 && bytes.get(offset - 1) == Some(&b'\r') && bytes.get(offset) == Some(&b'\n')
        {
            return Some(offset - 1..offset + 1);
        }
        None
    }

    pub fn outside_crlf_pair(&self, cursor: Cursor) -> Cursor {
        let forward = |offset: usize| match self.crlf_pair_at(offset) {
            Some(pair) if pair.start != offset => pair.end,
            _ => offset,
        };
        let backward = |offset: usize| match self.crlf_pair_at(offset) {
            Some(pair) if pair.start != offset => pair.start,
            _ => offset,
        };
        if !cursor.has_selection() {
            let at = BufferOffset(forward(cursor.position.get()));
            return Cursor {
                position: at,
                anchor: at,
                ..cursor
            };
        }
        let (start, end) = cursor.selection_range();
        let (start, end) = (
            BufferOffset(backward(start.get())),
            BufferOffset(forward(end.get())),
        );
        if cursor.reversed() {
            Cursor {
                position: start,
                anchor: end,
                ..cursor
            }
        } else {
            Cursor {
                position: end,
                anchor: start,
                ..cursor
            }
        }
    }

    pub(super) fn update_line_starts(&self, edits: &[Edit]) -> LineStarts {
        let mut line_starts = self.line_starts.to_full();
        for e in edits {
            let start_line = find_line(&line_starts, e.start);
            let end_line = find_line(&line_starts, e.end);
            let added_starts = compute_added_starts(e.start, &e.insert);
            let delta = e.insert.len() as isize - (e.end - e.start) as isize;

            for v in line_starts.iter_mut().skip(end_line + 1) {
                *v = v.saturating_add_signed(delta);
            }

            let mut next_starts = Vec::with_capacity(line_starts.len() + added_starts.len());
            if let Some(head) = line_starts.get(..=start_line) {
                next_starts.extend_from_slice(head);
            }
            next_starts.extend_from_slice(&added_starts);
            if let Some(tail) = line_starts.get(end_line + 1..) {
                next_starts.extend_from_slice(tail);
            }
            line_starts = next_starts;
        }
        LineStarts::from_full(line_starts)
    }
}

pub fn line_starts(content: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (i, b) in content.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

fn compute_added_starts(base_offset: usize, text: &str) -> Vec<usize> {
    let mut starts = Vec::new();
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            starts.push(base_offset + i + 1);
        }
    }
    starts
}

fn find_line(starts: &[usize], offset: usize) -> usize {
    if starts.is_empty() {
        return 0;
    }
    line_index_of(starts.get(1..).unwrap_or(&[]), offset)
}

fn line_index_of(subsequent_starts: &[usize], offset: usize) -> usize {
    subsequent_starts.partition_point(|&s| s <= offset)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::coords::VisualCol;
    use crate::cursor::CursorId;

    #[test]
    fn line_index() {
        let b = Buffer::new("line 1\nline 2\nline 3");
        assert_eq!(b.line_count(), 3);
        assert_eq!(b.line(0), "line 1");

        let bp = b.offset_to_line_col(10);
        assert_eq!(bp, BufferPoint { line: 1, col: 3 });

        let offset = b.caret_offset(bp);
        assert_eq!(offset, 10);
    }

    #[test]
    fn line_start_and_end_are_none_past_the_last_line() {
        let b = Buffer::new("only line");
        assert_eq!(b.line_count(), 1);
        assert_eq!(b.line_start(1), None);
        assert_eq!(b.line_end(1), None);
        assert_eq!(b.line_start(0), Some(0));
        assert_eq!(b.line_end(0), Some(9));
    }

    /// `col: 10` on the (non-last) middle line overshoots that line's own
    /// content; the offset must clamp to the LINE's end (5, the byte after
    /// "bb"), never to the whole buffer's end (8) and never past it
    /// unclamped. This distinguishes `bp.line == count - 1` from
    /// `bp.line != count - 1` (which would wrongly treat every non-last
    /// line as the last one) and `if offset > end` from `if offset == end`
    /// (which would only clamp an exact match, leaving an overshoot
    /// un-clamped and later force-snapped all the way to the buffer's own
    /// end instead of the line's).
    #[test]
    fn caret_offset_clamps_an_overshot_column_to_the_lines_own_end_not_the_buffers() {
        let b = Buffer::new("aa\nbb\ncc");
        let offset = b.caret_offset(BufferPoint { line: 1, col: 10 });
        assert_eq!(offset, 5, "must clamp to line 1's end, not content.len()");
    }

    #[test]
    fn line_content_end_stops_before_the_cr_of_a_crlf_line() {
        let b = Buffer::new("a\r\nb");
        assert_eq!(b.line_content_end(0), Some(1));
        assert_eq!(b.line_content_end(1), Some(4));
    }

    #[test]
    fn line_content_end_matches_line_end_on_a_bare_lf_line() {
        let b = Buffer::new("a\nb");
        assert_eq!(b.line_content_end(0), Some(1));
        assert_eq!(b.line_end(0), Some(1));
    }

    #[test]
    fn caret_offset_clamps_an_overshot_column_before_the_cr_of_a_crlf_line() {
        let b = Buffer::new("abcdef\r\nA\r\nC");
        assert_eq!(b.caret_offset(BufferPoint { line: 1, col: 5 }), 9);
    }

    #[test]
    fn caret_offset_on_an_empty_crlf_line_is_the_line_start() {
        let b = Buffer::new("a\r\n\r\nc");
        assert_eq!(b.caret_offset(BufferPoint { line: 1, col: 3 }), 3);
    }

    #[test]
    fn caret_offset_clamps_before_the_cr_after_multibyte_text() {
        let b = Buffer::new("x\r\nñé\r\nc");
        assert_eq!(b.caret_offset(BufferPoint { line: 1, col: 99 }), 7);
    }

    #[test]
    fn caret_offset_floors_a_mid_character_column_to_the_boundary() {
        let b = Buffer::new("ñ\r\n");
        assert_eq!(b.caret_offset(BufferPoint { line: 0, col: 1 }), 0);
    }

    #[test]
    fn caret_offset_keeps_a_lone_cr_before_lf_free_end_as_content() {
        let b = Buffer::new("a\nb\r");
        assert_eq!(b.caret_offset(BufferPoint { line: 1, col: 9 }), 4);
    }

    #[test]
    fn caret_offset_keeps_a_mid_line_lone_cr_as_content() {
        let b = Buffer::new("a\rb\nc");
        assert_eq!(b.caret_offset(BufferPoint { line: 0, col: 9 }), 3);
    }

    #[test]
    fn caret_offset_past_the_last_line_is_the_buffer_end() {
        let b = Buffer::new("a\r\nb\r\n");
        assert_eq!(b.caret_offset(BufferPoint { line: 7, col: 0 }), 6);
    }

    fn span(position: usize, anchor: usize) -> Cursor {
        Cursor {
            position: BufferOffset(position),
            anchor: BufferOffset(anchor),
            desired_col: VisualCol(0),
            id: CursorId::FIRST,
        }
    }

    fn snapped(b: &Buffer, position: usize, anchor: usize) -> (usize, usize) {
        let c = b.outside_crlf_pair(span(position, anchor));
        (c.position.get(), c.anchor.get())
    }

    #[test]
    fn outside_crlf_pair_moves_a_collapsed_caret_between_cr_and_lf_past_the_pair() {
        let b = Buffer::new("a\r\nb");
        assert_eq!(snapped(&b, 2, 2), (3, 3));
    }

    #[test]
    fn outside_crlf_pair_leaves_collapsed_carets_around_a_pair_and_beside_lone_cr_or_lf_alone() {
        let b = Buffer::new("a\r\nb\rc\nd\r");
        for offset in [0, 1, 3, 4, 5, 6, 7, 8, 9] {
            assert_eq!(
                snapped(&b, offset, offset),
                (offset, offset),
                "offset {offset}"
            );
        }
    }

    #[test]
    fn outside_crlf_pair_moves_a_selection_start_inside_a_pair_back_before_it() {
        let b = Buffer::new("a\r\nbc");
        assert_eq!(snapped(&b, 4, 2), (4, 1));
        assert_eq!(snapped(&b, 2, 4), (1, 4));
    }

    #[test]
    fn outside_crlf_pair_moves_a_selection_end_inside_a_pair_forward_past_it() {
        let b = Buffer::new("ab\r\nc");
        assert_eq!(snapped(&b, 3, 0), (4, 0));
        assert_eq!(snapped(&b, 0, 3), (0, 4));
    }

    #[test]
    fn outside_crlf_pair_keeps_the_identity_and_desired_column_of_the_cursor() {
        let b = Buffer::new("a\r\nb");
        let c = b.outside_crlf_pair(Cursor {
            desired_col: VisualCol(7),
            ..span(2, 2)
        });
        assert_eq!((c.desired_col, c.id), (VisualCol(7), CursorId::FIRST));
    }

    #[test]
    fn a_lone_trailing_cr_on_the_last_line_is_content_not_a_terminator() {
        let b = Buffer::new("a\r\nb\r");
        assert_eq!(b.line_content_end(1), Some(5));
        assert_eq!(b.line_terminator_range(1), Some(5..5));
    }

    #[test]
    fn line_terminator_range_reports_two_bytes_for_crlf_one_for_lf_none_when_unterminated() {
        let b = Buffer::new("a\r\nb\nc");
        assert_eq!(b.line_terminator_range(0), Some(1..3));
        assert_eq!(b.line_terminator_range(1), Some(4..5));
        assert_eq!(b.line_terminator_range(2), Some(6..6));
    }

    #[test]
    fn line_break_at_follows_the_carets_line_then_the_nearest_terminated_line_above() {
        let b = Buffer::new("a\r\nb\nc\r\nd");
        assert_eq!(b.line_break_at(0), "\r\n");
        assert_eq!(b.line_break_at(3), "\n");
        assert_eq!(b.line_break_at(5), "\r\n");
        assert_eq!(
            b.line_break_at(8),
            "\r\n",
            "an unterminated last line borrows the line above"
        );
        assert_eq!(b.line_break_at(usize::MAX), "\r\n");
    }

    #[test]
    fn line_break_at_defaults_to_lf_when_no_line_is_terminated() {
        assert_eq!(Buffer::new("").line_break_at(0), "\n");
        assert_eq!(Buffer::new("abc").line_break_at(2), "\n");
    }

    #[test]
    fn crlf_pair_at_matches_from_either_byte_and_not_a_bare_cr_or_lf() {
        let b = Buffer::new("a\r\nb");
        assert_eq!(b.crlf_pair_at(1), Some(1..3));
        assert_eq!(b.crlf_pair_at(2), Some(1..3));
        assert_eq!(b.crlf_pair_at(0), None);
        assert_eq!(b.crlf_pair_at(3), None);

        let bare_cr = Buffer::new("a\rb");
        assert_eq!(bare_cr.crlf_pair_at(1), None);
    }

    #[test]
    fn line_terminator_range_of_an_empty_first_line() {
        let b = Buffer::new("\nx");
        assert_eq!(b.line_terminator_range(0), Some(0..1));
    }

    #[test]
    fn a_bare_line_feed_is_not_a_crlf_pair() {
        let b = Buffer::new("a\nb");
        assert_eq!(b.crlf_pair_at(1), None);
    }

    #[test]
    fn update_line_starts_when_edit_end_lands_on_a_line_start_boundary() {
        let b = Buffer::new("aa\nbb\ncc");
        let replaced = b.replace(0, 3, "xx\nyy\n").expect("edit should apply");
        assert_eq!(replaced.content(), "xx\nyy\nbb\ncc");
        assert_eq!(replaced.line_count(), 4);
        assert_eq!(replaced.line(0), "xx");
        assert_eq!(replaced.line(1), "yy");
        assert_eq!(replaced.line(2), "bb");
        assert_eq!(replaced.line(3), "cc");
    }
}
