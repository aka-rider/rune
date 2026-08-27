use std::ops::Range;

use super::{Buffer, Edit};
use crate::coords::BufferPoint;

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

    pub fn line_col_to_offset(&self, bp: BufferPoint) -> usize {
        let count = self.line_starts.len();
        if bp.line >= count {
            return self.content.len();
        }
        let line_start = self.line_starts.get(bp.line).unwrap_or(0);
        let offset = line_start + bp.col;
        let end = if bp.line == count - 1 {
            self.content.len()
        } else {
            self.line_end(bp.line).unwrap_or(self.content.len())
        };
        let offset = if offset > end { end } else { offset };
        // `bp.col` is a BYTE column and callers routinely carry one across
        // lines — a remembered desired column, a click, a multicursor add.
        // Clamping to the line's end keeps it in RANGE but says nothing
        // about char boundaries, so a column measured on an ASCII line lands
        // mid-UTF-8 when replayed against a line holding wide characters.
        // Snapping here rather than at each call site is what makes an
        // out-of-boundary cursor unrepresentable instead of merely unlikely.
        super::snap_char_boundary(&self.content, offset)
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

    #[test]
    fn line_index() {
        let b = Buffer::new("line 1\nline 2\nline 3");
        assert_eq!(b.line_count(), 3);
        assert_eq!(b.line(0), "line 1");

        let bp = b.offset_to_line_col(10);
        assert_eq!(bp, BufferPoint { line: 1, col: 3 });

        let offset = b.line_col_to_offset(bp);
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
    fn line_col_to_offset_clamps_an_overshot_column_to_the_lines_own_end_not_the_buffers() {
        let b = Buffer::new("aa\nbb\ncc");
        let offset = b.line_col_to_offset(BufferPoint { line: 1, col: 10 });
        assert_eq!(offset, 5, "must clamp to line 1's end, not content.len()");
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
