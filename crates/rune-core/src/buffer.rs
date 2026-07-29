//! Immutable, value-semantics text buffer keyed by BYTE offsets (§1.5).
//! Ported from Go's buffer + line-index implementation.
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
//!   `clippy::indexing_slicing` lint — every `&content[a..b]` must come
//!   from a validated/clamped range, and the buffer's own methods ARE
//!   those clamping helpers, so nothing downstream ever indexes `content`
//!   directly.
//! - `Slice`/`Byte` panic in Go on an out-of-range argument (a bare Go
//!   slice/index expression). Per CONSTITUTION §1.3 ("halt, never panic")
//!   and the workspace's `clippy::panic`/`unwrap_used` deny-lints, the Rust
//!   equivalents return an empty/`None` fallback instead of panicking.

use crate::assert_invariant;
use crate::coords::BufferPoint;
use std::fmt;

/// One requested edit: replace the byte range `[start, end)` with `insert`.
/// Port of `buffer.go:18-23`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub insert: String,
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
    /// Two edits in the batch computed the identical post-edit `start` —
    /// the corruption path where a batch that is individually valid
    /// (non-overlapping, sorted) still collapses two `AppliedEdit`s onto
    /// one position once shifts are applied (e.g. two adjacent one-byte
    /// deletes). Refused here — at the one place both the write path
    /// (persisted journal rows) and the read-back path (`undo::reapply`)
    /// share — rather than left for a replayer to silently misorder.
    DuplicateEditStart { start: usize },
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
            BufferError::DuplicateEditStart { start } => {
                write!(f, "two edits collide on post-edit start {start}")
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

    /// Refuses non-UTF-8 bytes — the load-time refusal point. Port of
    /// `buffer.go:46-51`.
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

    pub fn insert(&self, offset: usize, text: &str) -> Result<Buffer, BufferError> {
        self.replace(offset, offset, text)
    }

    pub fn delete(&self, start: usize, end: usize) -> Result<Buffer, BufferError> {
        self.replace(start, end, "")
    }

    /// Convenience single-edit wrapper over `apply_edits`, used by
    /// `insert`/`delete`, tests, and programmatic edits that already know
    /// the range is valid. Surfaces `apply_edits`' error instead of
    /// silently returning the receiver unchanged — a caller that ignores
    /// the rejection can no longer mistake "nothing happened" for success
    /// (§1.3). Port of `buffer.go:81-92`, EXCEPT the start/end
    /// swap-if-reversed below: Go's `Buffer.Replace` has no such swap (it
    /// passes `start`/`end` straight through to `ApplyEdits`, so a reversed
    /// range is simply rejected as out-of-bounds) — the swap is ported from
    /// `textedit.ReplaceRange` (`edit_primitives.go:28-30`), which this
    /// method's actual callers (`insert`/`delete`, and arbitrary start/end
    /// from tests) rely on.
    pub fn replace(&self, start: usize, end: usize, text: &str) -> Result<Buffer, BufferError> {
        let (start, end) = if start > end {
            (end, start)
        } else {
            (start, end)
        };
        let edit = Edit {
            start,
            end,
            insert: text.to_string(),
        };
        let sorted = clone_and_sort_edits_descending(std::slice::from_ref(&edit));
        let (new_buf, _) = self.apply_edits(&sorted)?;
        Ok(new_buf)
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
            .map(|e| edit_delta(e.end - e.start, e.insert.len()))
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
                current_shift += edit_delta(e.end - e.start, e.insert.len());
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

            let prefix = self
                .content
                .get(last_end..e.start)
                .ok_or(BufferError::OutOfBounds {
                    start: last_end,
                    end: e.start,
                    len,
                })?;
            new_content.push_str(prefix);
            new_content.push_str(&e.insert);
            last_end = e.end;

            let start = (e.start as isize + shift).max(0) as usize;
            let deleted = self
                .content
                .get(e.start..e.end)
                .ok_or(BufferError::OutOfBounds {
                    start: e.start,
                    end: e.end,
                    len,
                })?
                .to_string();
            if let Some(slot) = applied.get_mut(i) {
                *slot = AppliedEdit {
                    start,
                    end: start + e.insert.len(),
                    deleted,
                    insert: e.insert.clone(),
                };
            }
        }
        let tail = self
            .content
            .get(last_end..)
            .ok_or(BufferError::OutOfBounds {
                start: last_end,
                end: len,
                len,
            })?;
        new_content.push_str(tail);

        if let Some(start) = duplicate_applied_start(&applied) {
            return Err(BufferError::DuplicateEditStart { start });
        }

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

    /// `None` when `n` is not a valid line index — `0` used to double as
    /// both "line 0 starts at byte 0" and "no such line" (§1.7 sentinel).
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
        // Go's original has a redundant `if bp.Line < len(starts)-1 { return
        // end }; return end` — both arms return `end`; simplified here.
        let offset = if offset > end { end } else { offset };
        // `bp.col` is a BYTE column (§1.5) and callers routinely carry one
        // across lines — a remembered desired column, a click, a multicursor
        // add. Clamping to the line's end keeps it in RANGE but says nothing
        // about char boundaries, so a column measured on an ASCII line lands
        // mid-UTF-8 when replayed against a line holding wide characters.
        // Snapping here rather than at each call site is what makes an
        // out-of-boundary cursor unrepresentable instead of merely unlikely
        // (§1.3 clamp, §1.5 bytes).
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
        assert_line_starts_invariant(&line_starts);
        line_starts
    }
}

/// The one place `insert_len - deleted_len` is computed — how many bytes a
/// single edit adds (negative for a net deletion). Re-derived independently
/// at five call sites before this chokepoint existed (`apply_edits` ×2,
/// `CursorSet::adjust_after_edit`, `adjust_after_batch_edits` ×2), each free
/// to make its own clamp decision. Takes plain lengths rather than an
/// `Edit`/`AppliedEdit` so both crate-side derivations (a range's
/// `end - start`, or an already-known `deleted.len()`) share it.
pub fn edit_delta(deleted_len: usize, insert_len: usize) -> isize {
    insert_len as isize - deleted_len as isize
}

/// The first `AppliedEdit::start` shared by more than one entry in
/// `applied`, if any — the corruption shape `BufferError::DuplicateEditStart`
/// exists to refuse: a batch that is individually valid (non-overlapping,
/// sorted) can still collapse two edits onto the identical post-edit
/// position (e.g. two adjacent one-byte deletes).
pub(crate) fn duplicate_applied_start(applied: &[AppliedEdit]) -> Option<usize> {
    let mut starts: Vec<usize> = applied.iter().map(|a| a.start).collect();
    starts.sort_unstable();
    starts
        .windows(2)
        .find(|w| w.first() == w.get(1))
        .and_then(|w| w.first().copied())
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
    assert_line_starts_invariant(&starts);
    starts
}

/// The invariant every `Buffer::line_starts` must uphold: non-empty, with
/// `line_starts[0] == 0`. Checked wherever `line_starts` is built or
/// rebuilt (`compute_line_starts`, `update_line_starts`) rather than only
/// documented — via the `STRICT_INVARIANTS`-gated `assert_invariant`
/// chokepoint, so a future change that reintroduces the malformed-empty
/// state (a derived `Default` producing `line_starts: vec![]`, the exact
/// shape `Buffer`'s manual `Default` above exists to prevent) is caught in
/// tests without an ordinary build ever paying for it.
fn assert_line_starts_invariant(line_starts: &[usize]) {
    assert_invariant(line_starts.first().copied() == Some(0), || {
        "line_starts must be non-empty and start with 0".to_string()
    });
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
            },
            Edit {
                start: 6,
                end: 11,
                insert: "b".to_string(),
            },
        ]);
        assert_eq!(err, Err(BufferError::EditsNotSortedOrOverlapping));

        // Overlapping (should fail).
        let err = b.apply_edits(&[
            Edit {
                start: 5,
                end: 10,
                insert: "a".to_string(),
            },
            Edit {
                start: 0,
                end: 6,
                insert: "b".to_string(),
            },
        ]);
        assert_eq!(err, Err(BufferError::EditsNotSortedOrOverlapping));

        // Correct (should pass).
        let ok = b.apply_edits(&[
            Edit {
                start: 6,
                end: 11,
                insert: "b".to_string(),
            },
            Edit {
                start: 0,
                end: 5,
                insert: "a".to_string(),
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
            },
            Edit {
                start: 6,
                end: 11,
                insert: "b".to_string(),
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

    #[test]
    fn line_start_and_end_are_none_past_the_last_line() {
        let b = Buffer::new("only line");
        assert_eq!(b.line_count(), 1);
        assert_eq!(b.line_start(1), None);
        assert_eq!(b.line_end(1), None);
        assert_eq!(b.line_start(0), Some(0));
        assert_eq!(b.line_end(0), Some(9));
    }

    /// [rune-core 1]: a batch that is individually valid (non-overlapping,
    /// sorted descending) but whose two edits compute the identical
    /// post-edit `start` — two adjacent one-byte deletes — is rejected by
    /// `apply_edits` itself rather than handed to a caller (`undo::reapply`,
    /// a replayed journal row) that would have to notice on its own.
    #[test]
    fn apply_edits_rejects_a_batch_whose_edits_collide_on_post_edit_start() {
        let b = Buffer::new("ab");
        let err = b.apply_edits(&[
            Edit {
                start: 1,
                end: 2,
                insert: String::new(),
            },
            Edit {
                start: 0,
                end: 1,
                insert: String::new(),
            },
        ]);
        assert_eq!(err, Err(BufferError::DuplicateEditStart { start: 0 }));
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
