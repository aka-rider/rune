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

use std::fmt;

mod lineindex;

/// One requested edit: replace the byte range `[start, end)` with `insert`.
/// Port of `buffer.go`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub insert: String,
}

/// The edit actually applied, in POST-edit coordinates, with the displaced
/// text kept for inversion (undo). Port of `buffer.go`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppliedEdit {
    pub start: usize,
    pub end: usize,
    pub deleted: String,
    pub insert: String,
}

/// Why an edit batch was rejected. `ApplyEdits` never panics — every
/// rejected edit surfaces one of these instead (§1.3). Port of the error
/// cases in `buffer.go`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BufferError {
    /// `Buffer::from_bytes` was given bytes that are not valid UTF-8.
    InvalidUtf8,
    /// The edit batch was not sorted descending by `start` and
    /// non-overlapping (`buffer.go`).
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
/// Port of `buffer.go`.
///
/// Invariant: `line_starts` is never empty and `line_starts[0] == 0` —
/// every method below assumes it (`line_start`/`line_end`/`find_line`/
/// `update_line_starts` all read `line_starts` under this assumption). Go's
/// `getLineStarts()` (`lineindex.go`) nil-guards a zero-valued
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
    /// Port of `buffer.go`.
    pub fn new(content: impl Into<String>) -> Buffer {
        let content = content.into();
        let line_starts = lineindex::compute_line_starts(&content);
        Buffer {
            content,
            line_starts,
            version: 1,
        }
    }

    /// Refuses non-UTF-8 bytes — the load-time refusal point. Port of
    /// `buffer.go`.
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
    /// (§1.3). Port of `buffer.go`, EXCEPT the start/end
    /// swap-if-reversed below: Go's `Buffer.Replace` has no such swap (it
    /// passes `start`/`end` straight through to `ApplyEdits`, so a reversed
    /// range is simply rejected as out-of-bounds) — the swap is ported from
    /// `textedit.ReplaceRange` (`edit_primitives.go`), which this
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
    /// of `buffer.go`.
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

/// Port of `buffer.go`.
pub fn is_sorted_descending_non_overlapping(edits: &[Edit]) -> bool {
    edits.windows(2).all(|w| match (w.first(), w.get(1)) {
        (Some(a), Some(b)) => a.start >= b.end,
        _ => true,
    })
}

/// Port of `buffer.go`. Rust's `sort_by` is stable (matches Go's
/// `sort.Slice` intent, though Go's is not itself guaranteed stable — this
/// is a strictly more deterministic tie-break, not a behavior change for
/// any distinguishable `(start, end)` pair).
pub fn clone_and_sort_edits_descending(edits: &[Edit]) -> Vec<Edit> {
    let mut cloned = edits.to_vec();
    cloned.sort_by(|a, b| b.start.cmp(&a.start).then(b.end.cmp(&a.end)));
    cloned
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
}
