//! `Buffer`'s line-index / coordinate-conversion side: the `line_starts`
//! index itself, offset<->`BufferPoint` conversion, and the incremental
//! rebuild `apply_edits` drives on every edit.

use super::{Buffer, Edit};
use crate::assert_invariant;
use crate::coords::BufferPoint;

impl Buffer {
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// `None` when `n` is not a valid line index — `0` used to double as
    /// both "line 0 starts at byte 0" and "no such line".
    pub fn line_start(&self, n: usize) -> Option<usize> {
        self.line_starts.get(n).copied()
    }

    /// `None` when `n` is not a valid line index (see `line_start`).
    pub fn line_end(&self, n: usize) -> Option<usize> {
        let count = self.line_starts.len();
        if n >= count {
            return None;
        }
        if n == count - 1 {
            return Some(self.content.len());
        }
        Some(self.line_starts.get(n + 1).copied()?.saturating_sub(1))
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
        let line = find_line(&self.line_starts, offset);
        let line_start = self.line_starts.get(line).copied().unwrap_or(0);
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
        let line_start = self.line_starts.get(bp.line).copied().unwrap_or(0);
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
        self.floor_char_boundary(offset)
    }

    /// The largest offset `<= offset` that is a valid char boundary.
    fn floor_char_boundary(&self, offset: usize) -> usize {
        let mut o = offset.min(self.content.len());
        while o > 0 && !self.content.is_char_boundary(o) {
            o -= 1;
        }
        o
    }

    /// An incremental `line_starts` rebuild scanning `edits` right-to-left
    /// (descending `start`), so each edit only touches the portion of
    /// `line_starts` it actually displaces.
    pub(super) fn update_line_starts(&self, edits: &[Edit]) -> Vec<usize> {
        let mut line_starts = self.line_starts.clone();
        for e in edits {
            let start_line = find_line(&line_starts, e.start);
            let end_line = find_line(&line_starts, e.end);
            let added_starts = compute_added_starts(e.start, &e.insert);
            let delta = e.insert.len() as isize - (e.end - e.start) as isize;

            for v in line_starts.iter_mut().skip(end_line + 1) {
                *v = (*v as isize + delta).max(0) as usize;
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
        assert_line_starts_invariant(&line_starts);
        line_starts
    }
}

pub(super) fn compute_line_starts(content: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (i, b) in content.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    assert_line_starts_invariant(&starts);
    starts
}

/// The invariant every `Buffer::line_starts` must uphold: non-empty, with
/// `line_starts[0] == 0`. Checked wherever `line_starts` is built or
/// rebuilt (`compute_line_starts`, `update_line_starts`) rather than only
/// documented — via the `assert_invariant` chokepoint, so a future change
/// that reintroduces the malformed-empty state (a derived `Default`
/// producing `line_starts: vec![]`, the exact shape `Buffer`'s manual
/// `Default` above exists to prevent) is caught in tests without an
/// ordinary build ever paying for it.
fn assert_line_starts_invariant(line_starts: &[usize]) {
    assert_invariant!(line_starts.first().copied() == Some(0), || {
        "line_starts must be non-empty and start with 0".to_string()
    });
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

/// The line index `i` such that `starts[i] <= offset < starts[i+1]` (or the
/// last line if `offset` is at or past the final line start).
fn find_line(starts: &[usize], offset: usize) -> usize {
    if starts.is_empty() {
        return 0;
    }
    let idx = starts.partition_point(|&s| s <= offset);
    idx.saturating_sub(1)
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

        let bp = b.offset_to_line_col(10); // "line 1\nlin|e 2"
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

    /// [rune-core 15]: the incremental `update_line_starts` rebuild has a
    /// subtle boundary when an edit's `end` lands EXACTLY on an existing
    /// `line_starts[m]` — `find_line` must treat that boundary consistently
    /// on both sides of the edit so no line start is duplicated or dropped.
    #[test]
    fn update_line_starts_when_edit_end_lands_on_a_line_start_boundary() {
        let b = Buffer::new("aa\nbb\ncc");
        // line_starts == [0, 3, 6]. Replace exactly [0, 3) — end lands on
        // the second line start — with a two-line replacement.
        let replaced = b.replace(0, 3, "xx\nyy\n").expect("edit should apply");
        assert_eq!(replaced.content(), "xx\nyy\nbb\ncc");
        assert_eq!(replaced.line_count(), 4);
        assert_eq!(replaced.line(0), "xx");
        assert_eq!(replaced.line(1), "yy");
        assert_eq!(replaced.line(2), "bb");
        assert_eq!(replaced.line(3), "cc");
    }
}
