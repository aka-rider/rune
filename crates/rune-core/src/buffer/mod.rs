//! Immutable, value-semantics text buffer keyed by BYTE offsets.
//!
//! Type-driven invariants (each removes an illegal state rather than
//! guarding it at runtime):
//! - `Edit`/`AppliedEdit` offsets are `usize` — a negative offset is
//!   unrepresentable.
//! - `Edit::insert` is a Rust `String`, so UTF-8 validity is enforced by
//!   the type itself.
//! - Every access that would need `[]` indexing goes through
//!   `.get()`/`.get_mut()` instead, per the workspace's
//!   `clippy::indexing_slicing` lint — every `&content[a..b]` must come
//!   from a validated/clamped range, and the buffer's own methods ARE
//!   those clamping helpers, so nothing downstream ever indexes `content`
//!   directly.
//! - An out-of-range `slice`/`byte` access returns an empty/`None`
//!   fallback instead of panicking, per the workspace's
//!   `clippy::panic`/`unwrap_used` deny-lints — a panic would take the
//!   unsaved buffer down with it.

use std::fmt;

mod lineindex;

use lineindex::LineStarts;
pub use lineindex::line_starts;

pub(crate) fn check_char_boundary(content: &str, offset: usize) -> Result<(), BufferError> {
    if content.is_char_boundary(offset) {
        Ok(())
    } else {
        Err(BufferError::SplitsRune { offset })
    }
}

pub(crate) fn snap_char_boundary(content: &str, offset: usize) -> usize {
    content.floor_char_boundary(offset)
}

/// One requested edit: replace the byte range `[start, end)` with `insert`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub insert: String,
}

/// The edit actually applied, in POST-edit coordinates, with the displaced
/// text kept for inversion (undo).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppliedEdit {
    pub start: usize,
    pub end: usize,
    pub deleted: String,
    pub insert: String,
}

/// Why an edit batch was rejected. `ApplyEdits` never panics — every
/// rejected edit surfaces one of these instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BufferError {
    /// `Buffer::from_bytes` was given bytes that are not valid UTF-8.
    InvalidUtf8,
    /// The edit batch was not sorted descending by `start` and
    /// non-overlapping.
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
/// `tests/buffer_roundtrip.rs`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Buffer {
    content: String,
    line_starts: LineStarts,
    version: u64,
}

impl Default for Buffer {
    fn default() -> Buffer {
        Buffer::new("")
    }
}

impl Buffer {
    pub fn new(content: impl Into<String>) -> Buffer {
        let content = content.into();
        let line_starts = LineStarts::from_full(lineindex::line_starts(&content));
        Buffer {
            content,
            line_starts,
            version: 1,
        }
    }

    /// Refuses non-UTF-8 bytes — the load-time refusal point.
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

    /// Advances this buffer's version to be strictly greater than `floor`,
    /// touching no byte of its content — for a caller that replaces one
    /// buffer's content with another's under the SAME identity (a document
    /// id kept across the swap) and needs every consumer gated on
    /// `version()` to see that as a genuine change, even when both buffers
    /// independently started at version 1. A no-op once this buffer's own
    /// version already exceeds `floor`.
    pub fn advance_past(mut self, floor: u64) -> Buffer {
        self.version = self.version.max(floor.saturating_add(1));
        self
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    /// Returns `None` instead of panicking when `[start, end)` is not a
    /// valid range on `content` — out of bounds, reversed (`start > end`),
    /// or splitting a multi-byte char — consistent with `byte`/`rune_at`
    /// below (see module docs). Deliberately NOT `""` on failure: an empty
    /// string is indistinguishable from a legitimately empty slice, and a
    /// caller recording displaced bytes must never mistake a lost range for
    /// an intentional no-op — a `None` they mishandle is at least a visible
    /// bug, not a silent "nothing was displaced".
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
    /// the rejection can no longer mistake "nothing happened" for success.
    /// A reversed `start`/`end` is rejected by `apply_edits` as
    /// `OutOfBounds`, same as any other malformed range.
    pub fn replace(&self, start: usize, end: usize, text: &str) -> Result<Buffer, BufferError> {
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
    /// `clone_and_sort_edits_descending`) — validated, never assumed.
    pub fn apply_edits(&self, edits: &[Edit]) -> Result<(Buffer, Vec<AppliedEdit>), BufferError> {
        if edits.is_empty() {
            return Ok((self.clone(), Vec::new()));
        }

        validate_edit_batch(&self.content, edits)?;

        let (new_content, applied) = self.build_edited_content(edits)?;

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

    fn build_edited_content(
        &self,
        edits: &[Edit],
    ) -> Result<(String, Vec<AppliedEdit>), BufferError> {
        let len = self.content.len();

        let net_change: isize = edits
            .iter()
            .map(|e| edit_delta(e.end - e.start, e.insert.len()))
            .sum();
        let cap = len.saturating_add_signed(net_change);
        let mut new_content = String::with_capacity(cap);

        // Precompute each edit's cumulative shift, scanning right-to-left
        // (descending `start` order matches array order here).
        let mut shifts = vec![0isize; edits.len()];
        let mut current_shift: isize = 0;
        for (e, slot) in edits.iter().zip(shifts.iter_mut()).rev() {
            *slot = current_shift;
            current_shift += edit_delta(e.end - e.start, e.insert.len());
        }

        let mut applied: Vec<AppliedEdit> = Vec::with_capacity(edits.len());
        applied.resize_with(edits.len(), AppliedEdit::default);

        // Walk left-to-right (ascending `start`) to build the new content,
        // which is why this loop also runs in reverse over the
        // descending-sorted `edits` array.
        let mut last_end = 0usize;
        for (i, (e, shift)) in edits.iter().zip(&shifts).enumerate().rev() {
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

            let start = e.start.saturating_add_signed(*shift);
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

        Ok((new_content, applied))
    }
}

fn validate_edit_batch(content: &str, edits: &[Edit]) -> Result<(), BufferError> {
    if !is_sorted_descending_non_overlapping(edits) {
        return Err(BufferError::EditsNotSortedOrOverlapping);
    }

    let len = content.len();
    for e in edits {
        if e.end > len || e.start > e.end {
            return Err(BufferError::OutOfBounds {
                start: e.start,
                end: e.end,
                len,
            });
        }
        check_char_boundary(content, e.start)?;
        check_char_boundary(content, e.end)?;
    }
    Ok(())
}

/// The one place `insert_len - deleted_len` is computed — how many bytes a
/// single edit adds (negative for a net deletion). Takes plain lengths
/// rather than an `Edit`/`AppliedEdit` so both crate-side derivations (a
/// range's `end - start`, or an already-known `deleted.len()`) share it.
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

pub fn is_sorted_descending_non_overlapping(edits: &[Edit]) -> bool {
    edits.windows(2).all(|w| match (w.first(), w.get(1)) {
        (Some(a), Some(b)) => a.start >= b.end,
        _ => true,
    })
}

/// `sort_by` is stable — ties break deterministically for any
/// distinguishable `(start, end)` pair.
pub fn clone_and_sort_edits_descending(edits: &[Edit]) -> Vec<Edit> {
    let mut cloned = edits.to_vec();
    cloned.sort_by(|a, b| b.start.cmp(&a.start).then(b.end.cmp(&a.end)));
    cloned
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn from_bytes() {
        let b = Buffer::from_bytes(b"Hello \xe2\x98\xba World".to_vec())
            .expect("valid utf-8 should not error");
        assert_eq!(b.content(), "Hello \u{263a} World");

        let err = Buffer::from_bytes(vec![0xff, 0xfe]);
        assert_eq!(err, Err(BufferError::InvalidUtf8));
    }

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
