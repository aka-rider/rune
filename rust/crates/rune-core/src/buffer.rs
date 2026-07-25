//! Immutable, value-semantics text buffer keyed by BYTE offsets (§1.5).
//! Port of `pkg/editor/buffer/buffer.go` and `pkg/editor/buffer/lineindex.go`.
//!
//! Deliberate, type-driven departures from the Go original (each removes an
//! illegal state rather than guarding it at runtime):
//! - `Edit`/`AppliedEdit` offsets are `usize`, not Go's signed `int` — a
//!   negative offset is now unrepresentable, so the Go `e.Start < 0` guard
//!   has no Rust equivalent to port.
//! - `Edit::insert` is a Rust `String`, which is a UTF-8 invariant enforced
//!   by the type itself — Go's `utf8.ValidString(e.Insert)` runtime check
//!   has no reachable failure case to port.
//! - Every access that would use `[]` indexing in the Go original goes
//!   through `.get()`/`.get_mut()` here instead, per the workspace's
//!   `clippy::indexing_slicing` lint (Gotchas: "every `&content[a..b]` must
//!   come from validated/clamped ranges; use the buffer's clamping
//!   helpers") — the buffer's own methods ARE those clamping helpers, so
//!   nothing downstream ever indexes `content` directly.
//! - `Slice`/`Byte` panic in Go on an out-of-range argument (a bare Go
//!   slice/index expression). Per CONSTITUTION §1.3 ("halt, never panic")
//!   and the workspace's `clippy::panic`/`unwrap_used` deny-lints, the Rust
//!   equivalents return an empty/`None` fallback instead of panicking.

use crate::coords::BufferPoint;
use std::fmt;

/// One requested edit: replace the byte range `[start, end)` with `insert`.
/// `cursor_id`, when non-zero, is the id of the `cursor::Cursor` whose
/// command produced this edit (0 means no single owning cursor — e.g. a
/// programmatic replace). Port of `buffer.go:18-23`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub insert: String,
    pub cursor_id: u32,
}

/// The edit actually applied, in POST-edit coordinates, with the displaced
/// text kept for inversion (undo). Port of `buffer.go:25-30`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppliedEdit {
    pub start: usize,
    pub end: usize,
    pub deleted: String,
    pub insert: String,
}

/// Why an edit batch was rejected. `ApplyEdits` never panics — every
/// rejected edit surfaces one of these instead (§1.3). Port of the error
/// cases in `buffer.go:115-137`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BufferError {
    /// `Buffer::from_bytes` was given bytes that are not valid UTF-8.
    InvalidUtf8,
    /// The edit batch was not sorted descending by `start` and
    /// non-overlapping (`buffer.go:94-101`).
    EditsNotSortedOrOverlapping,
    /// An edit's range does not fit the live buffer.
    OutOfBounds {
        start: usize,
        end: usize,
        len: usize,
    },
    /// An edit's `start` or `end` falls inside a multi-byte UTF-8 character.
    SplitsRune { offset: usize },
}

impl fmt::Display for BufferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BufferError::InvalidUtf8 => write!(f, "invalid UTF-8 sequence"),
            BufferError::EditsNotSortedOrOverlapping => {
                write!(f, "edits must be non-overlapping and sorted descending")
            }
            BufferError::OutOfBounds { start, end, len } => {
                write!(f, "edit out of bounds: [{start},{end}) len={len}")
            }
            BufferError::SplitsRune { offset } => {
                write!(f, "edit splits a rune at byte offset {offset}")
            }
        }
    }
}

impl std::error::Error for BufferError {}

/// An immutable snapshot of document content. Every mutation returns a new
/// `Buffer`; the receiver is untouched (fuzz-proven in
/// `tests/buffer_roundtrip.rs`, port of `FuzzBufferSnapshotImmutability`).
/// Port of `buffer.go:32-36`.
///
/// Invariant: `line_starts` is never empty and `line_starts[0] == 0` —
/// every method below assumes it (`line_start`/`line_end`/`find_line`/
/// `update_line_starts` all read `line_starts` under this assumption). Go's
/// `getLineStarts()` (`lineindex.go:15-20`) nil-guards a zero-valued
/// `Buffer{}` back to `[0]` for exactly this reason. A derived
/// `#[derive(Default)]` would produce `line_starts: vec![]` instead — an
/// unrepresentable-by-construction fix: `Buffer` gets a manual `Default`
/// impl below that routes through `Buffer::new("")`, so no `Buffer` can
/// ever exist with a malformed line index in the first place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Buffer {
    content: String,
    line_starts: Vec<usize>,
    version: u64,
}

impl Default for Buffer {
    fn default() -> Self {
        Buffer::new("")
    }
}

impl Buffer {
    /// Port of `buffer.go:38-44`.
    pub fn new(content: impl Into<String>) -> Buffer {
        let content = content.into();
        let line_starts = compute_line_starts(&content);
        Buffer {
            content,
            line_starts,
            version: 1,
        }
    }

    /// Refuses non-UTF-8 bytes — the load-time refusal point (§0, plan
    /// decision 4). Port of `buffer.go:46-51`.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Buffer, BufferError> {
        let content = String::from_utf8(bytes).map_err(|_| BufferError::InvalidUtf8)?;
        Ok(Buffer::new(content))
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn len(&self) -> usize {
        self.content.len()
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    /// Returns `None` instead of panicking when `[start, end)` is not a
    /// valid range on `content` — out of bounds, reversed (`start > end`),
    /// or splitting a multi-byte char — consistent with `byte`/`rune_at`
    /// below (see module docs — a deliberate deviation from Go's panicking
    /// `content[start:end]`). Deliberately NOT `""` on failure: an empty
    /// string is indistinguishable from a legitimately empty slice, which
    /// would be a §1.4.10 hazard for any caller recording displaced bytes
    /// (a `None` they mishandle is at least a visible bug, not a silent
    /// "nothing was displaced").
    pub fn slice(&self, start: usize, end: usize) -> Option<&str> {
        self.content.get(start..end)
    }

    pub fn byte(&self, offset: usize) -> Option<u8> {
        self.content.as_bytes().get(offset).copied()
    }

    /// The rune starting at `offset` and its UTF-8 byte width, or `None` if
    /// `offset` is not a valid char boundary within the content.
    pub fn rune_at(&self, offset: usize) -> Option<(char, usize)> {
        let c = self.content.get(offset..)?.chars().next()?;
        Some((c, c.len_utf8()))
    }

    pub fn insert(&self, offset: usize, text: &str) -> Buffer {
        self.replace(offset, offset, text)
    }

    pub fn delete(&self, start: usize, end: usize) -> Buffer {
        self.replace(start, end, "")
    }

    /// Convenience single-edit wrapper over `apply_edits`, used by
    /// `insert`/`delete`, tests, and programmatic edits that already know
    /// the range is valid. Discards `apply_edits`' error and returns the
    /// receiver's content unchanged on a rejected edit — `apply_edits` is
    /// the primitive that surfaces the error (§1.3). Port of
    /// `buffer.go:81-92`, EXCEPT the start/end swap-if-reversed below: Go's
    /// `Buffer.Replace` has no such swap (it passes `start`/`end` straight
    /// through to `ApplyEdits`, so a reversed range is simply rejected as
    /// out-of-bounds) — the swap is ported from `textedit.ReplaceRange`
    /// (`edit_primitives.go:28-30`), which this method's actual callers
    /// (`insert`/`delete`, and arbitrary start/end from tests) rely on.
    pub fn replace(&self, start: usize, end: usize, text: &str) -> Buffer {
        let (start, end) = if start > end {
            (end, start)
        } else {
            (start, end)
        };
        let edit = Edit {
            start,
            end,
            insert: text.to_string(),
            cursor_id: 0,
        };
        let sorted = clone_and_sort_edits_descending(std::slice::from_ref(&edit));
        match self.apply_edits(&sorted) {
            Ok((new_buf, _)) => new_buf,
            Err(_) => self.clone(),
        }
    }

    /// Apply a batch of edits atomically. `edits` must already be sorted
    /// descending by `start` and non-overlapping (see
    /// `clone_and_sort_edits_descending`) — validated, never assumed. Port
    /// of `buffer.go:115-181`.
    pub fn apply_edits(&self, edits: &[Edit]) -> Result<(Buffer, Vec<AppliedEdit>), BufferError> {
        if edits.is_empty() {
            return Ok((self.clone(), Vec::new()));
        }

        if !is_sorted_descending_non_overlapping(edits) {
            return Err(BufferError::EditsNotSortedOrOverlapping);
        }

        let len = self.content.len();
        for e in edits {
            if e.end > len || e.start > e.end {
                return Err(BufferError::OutOfBounds {
                    start: e.start,
                    end: e.end,
                    len,
                });
            }
            if !self.content.is_char_boundary(e.start) {
                return Err(BufferError::SplitsRune { offset: e.start });
            }
            if !self.content.is_char_boundary(e.end) {
                return Err(BufferError::SplitsRune { offset: e.end });
            }
        }

        let net_change: isize = edits
            .iter()
            .map(|e| e.insert.len() as isize - (e.end - e.start) as isize)
            .sum();
        let cap = (len as isize + net_change).max(0) as usize;
        let mut new_content = String::with_capacity(cap);

        // Precompute each edit's cumulative shift, scanning right-to-left
        // (descending `start` order matches array order here).
        let mut shifts = vec![0isize; edits.len()];
        let mut current_shift: isize = 0;
        for i in (0..edits.len()).rev() {
            if let (Some(e), Some(slot)) = (edits.get(i), shifts.get_mut(i)) {
                *slot = current_shift;
                current_shift += e.insert.len() as isize - (e.end - e.start) as isize;
            }
        }

        let mut applied: Vec<AppliedEdit> = Vec::with_capacity(edits.len());
        applied.resize_with(edits.len(), AppliedEdit::default);

        // Walk left-to-right (ascending `start`) to build the new content,
        // which is why this loop also runs in reverse over the
        // descending-sorted `edits` array.
        let mut last_end = 0usize;
        for i in (0..edits.len()).rev() {
            let e = match edits.get(i) {
                Some(e) => e,
                None => continue,
            };
            let shift = shifts.get(i).copied().unwrap_or(0);

            new_content.push_str(self.content.get(last_end..e.start).unwrap_or(""));
            new_content.push_str(&e.insert);
            last_end = e.end;

            let start = (e.start as isize + shift).max(0) as usize;
            let deleted = self.content.get(e.start..e.end).unwrap_or("").to_string();
            if let Some(slot) = applied.get_mut(i) {
                *slot = AppliedEdit {
                    start,
                    end: start + e.insert.len(),
                    deleted,
                    insert: e.insert.clone(),
                };
            }
        }
        new_content.push_str(self.content.get(last_end..).unwrap_or(""));

        let new_line_starts = self.update_line_starts(edits);

        Ok((
            Buffer {
                content: new_content,
                line_starts: new_line_starts,
                version: self.version + 1,
            },
            applied,
        ))
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    pub fn line_start(&self, n: usize) -> usize {
        self.line_starts.get(n).copied().unwrap_or(0)
    }

    pub fn line_end(&self, n: usize) -> usize {
        let count = self.line_starts.len();
        if n >= count {
            return 0;
        }
        if n == count - 1 {
            return self.content.len();
        }
        self.line_starts
            .get(n + 1)
            .copied()
            .unwrap_or(0)
            .saturating_sub(1)
    }

    pub fn line(&self, n: usize) -> &str {
        let start = self.line_start(n);
        let end = self.line_end(n);
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
            self.line_end(bp.line)
        };
        // Go's original has a redundant `if bp.Line < len(starts)-1 { return
        // end }; return end` — both arms return `end`; simplified here.
        if offset > end { end } else { offset }
    }

    /// Port of `lineindex.go:22-49`: an incremental `line_starts` rebuild
    /// scanning `edits` right-to-left (descending `start`), so each edit
    /// only touches the portion of `line_starts` it actually displaces.
    fn update_line_starts(&self, edits: &[Edit]) -> Vec<usize> {
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
        debug_assert_line_starts_invariant(&line_starts);
        line_starts
    }
}

/// Port of `buffer.go:94-101`.
pub fn is_sorted_descending_non_overlapping(edits: &[Edit]) -> bool {
    edits.windows(2).all(|w| match (w.first(), w.get(1)) {
        (Some(a), Some(b)) => a.start >= b.end,
        _ => true,
    })
}

/// Port of `buffer.go:103-113`. Rust's `sort_by` is stable (matches Go's
/// `sort.Slice` intent, though Go's is not itself guaranteed stable — this
/// is a strictly more deterministic tie-break, not a behavior change for
/// any distinguishable `(start, end)` pair).
pub fn clone_and_sort_edits_descending(edits: &[Edit]) -> Vec<Edit> {
    let mut cloned = edits.to_vec();
    cloned.sort_by(|a, b| b.start.cmp(&a.start).then(b.end.cmp(&a.end)));
    cloned
}

/// Port of `lineindex.go:5-13`.
fn compute_line_starts(content: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (i, b) in content.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    debug_assert_line_starts_invariant(&starts);
    starts
}

/// The invariant every `Buffer::line_starts` must uphold: non-empty, with
/// `line_starts[0] == 0`. Checked wherever `line_starts` is built or
/// rebuilt (`compute_line_starts`, `update_line_starts`) rather than only
/// documented — a `debug_assert!` catches a future change that reintroduces
/// the empty-index state finding 1 fixed (`Buffer::default()` used to
/// derive `line_starts: vec![]`).
fn debug_assert_line_starts_invariant(line_starts: &[usize]) {
    debug_assert_eq!(
        line_starts.first().copied(),
        Some(0),
        "line_starts must be non-empty and start with 0"
    );
}

/// Port of `lineindex.go:51-59`.
fn compute_added_starts(base_offset: usize, text: &str) -> Vec<usize> {
    let mut starts = Vec::new();
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            starts.push(base_offset + i + 1);
        }
    }
    starts
}

/// Port of `lineindex.go:61-69`: the line index `i` such that
/// `starts[i] <= offset < starts[i+1]` (or the last line if `offset` is at
/// or past the final line start).
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

    /// Port of `TestBuffer_FromBytes`.
    #[test]
    fn from_bytes() {
        let b = Buffer::from_bytes(b"Hello \xe2\x98\xba World".to_vec())
            .expect("valid utf-8 should not error");
        assert_eq!(b.content(), "Hello \u{263a} World");

        let err = Buffer::from_bytes(vec![0xff, 0xfe]);
        assert_eq!(err, Err(BufferError::InvalidUtf8));
    }

    /// Port of `TestBuffer_ApplyEdits_DescendingOrderAndOverlap`.
    #[test]
    fn apply_edits_descending_order_and_overlap() {
        let b = Buffer::new("hello world");

        // Ascending order (should fail).
        let err = b.apply_edits(&[
            Edit {
                start: 0,
                end: 5,
                insert: "a".to_string(),
                cursor_id: 0,
            },
            Edit {
                start: 6,
                end: 11,
                insert: "b".to_string(),
                cursor_id: 0,
            },
        ]);
        assert_eq!(err, Err(BufferError::EditsNotSortedOrOverlapping));

        // Overlapping (should fail).
        let err = b.apply_edits(&[
            Edit {
                start: 5,
                end: 10,
                insert: "a".to_string(),
                cursor_id: 0,
            },
            Edit {
                start: 0,
                end: 6,
                insert: "b".to_string(),
                cursor_id: 0,
            },
        ]);
        assert_eq!(err, Err(BufferError::EditsNotSortedOrOverlapping));

        // Correct (should pass).
        let ok = b.apply_edits(&[
            Edit {
                start: 6,
                end: 11,
                insert: "b".to_string(),
                cursor_id: 0,
            },
            Edit {
                start: 0,
                end: 5,
                insert: "a".to_string(),
                cursor_id: 0,
            },
        ]);
        assert!(ok.is_ok());
    }

    /// Port of `TestBuffer_CloneAndSortEditsDescending`.
    #[test]
    fn clone_and_sort_edits_descending_test() {
        let edits = vec![
            Edit {
                start: 0,
                end: 5,
                insert: "a".to_string(),
                cursor_id: 0,
            },
            Edit {
                start: 6,
                end: 11,
                insert: "b".to_string(),
                cursor_id: 0,
            },
        ];
        let sorted = clone_and_sort_edits_descending(&edits);

        assert_eq!(edits[0].start, 0, "original slice was mutated");
        assert_eq!(sorted[0].start, 6);
        assert_eq!(sorted[1].start, 0);
    }

    /// Port of `TestBuffer_LineIndex`.
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
}
