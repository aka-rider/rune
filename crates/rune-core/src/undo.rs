//! In-memory undo journal, SQLite-shaped for Phase 2 (the durable store adds
//! persistence behind the same peek-then-commit shape, not a new one).
//! Ports Go's inverse/reapply edit-primitive formulas and its
//! peek-then-commit journal discipline.

use crate::buffer::{
    AppliedEdit, Buffer, BufferError, Edit, clone_and_sort_edits_descending,
    duplicate_applied_start,
};
use crate::cursor::Cursor;

/// Build the inverse edit batch for an applied-edit batch (undo): each
/// edit's insert becomes a delete range, and its deleted text becomes the
/// new insert. Port of the edit construction in
/// `edit_primitives.go:51-60` (`ApplyInverse`).
///
/// Returns `BufferError::OutOfBounds` instead of panicking if
/// `ae.start + ae.insert.len()` would overflow `usize`. Unreachable from
/// any edit `apply_edits` itself produced (every real `start`/`len` is
/// bounded by a live document's byte length), but `edits` is `pub fn`
/// -reachable data — Phase 2 will feed it back from SQLite — and §1.3
/// forbids panicking on adversarial input regardless of how unreachable it
/// is today.
pub fn inverse_edits(edits: &[AppliedEdit]) -> Result<Vec<Edit>, BufferError> {
    let mut raw = Vec::with_capacity(edits.len());
    for ae in edits {
        let end = ae
            .start
            .checked_add(ae.insert.len())
            .ok_or(BufferError::OutOfBounds {
                start: ae.start,
                end: usize::MAX,
                len: usize::MAX,
            })?;
        raw.push(Edit {
            start: ae.start,
            end,
            insert: ae.deleted.clone(),
        });
    }
    Ok(clone_and_sort_edits_descending(&raw))
}

/// Apply the inverse of `edits` to `buf` (undo). All-or-nothing: on error
/// `buf` is untouched by the caller — the journal position must stay put
/// too (§1.4.8, `workspace_undo.go:31-46`). Port of
/// `edit_primitives.go:51-68`.
pub fn apply_inverse(buf: &Buffer, edits: &[AppliedEdit]) -> Result<Buffer, BufferError> {
    let inverse = inverse_edits(edits)?;
    let (new_buf, _) = buf.apply_edits(&inverse)?;
    Ok(new_buf)
}

/// Reapply `edits` forward against `buf` (redo), one edit at a time,
/// ascending by `start`, against a running copy. `AppliedEdit::start`
/// carries a baked-in cumulative shift that is only valid one edit at a
/// time against the running buffer — see `edit_primitives.go:70-79` for
/// why batching them would be wrong. All-or-nothing: any edit's failure
/// returns the error and the original `buf` is never touched. Port of
/// `edit_primitives.go:86-110`.
///
/// Real invariant: no two edits in `edits` may share the same `start` (in
/// the post-edit coordinate space `AppliedEdit::start` lives in) — a tie
/// makes the ascending sort's replay order depend on which edit the sort
/// happened to place first, silently reordering an insert against an
/// adjacent delete. This invariant is NOT guaranteed by `CursorSet::merge`
/// alone: `merge` only coalesces cursors whose SELECTIONS touch, but two
/// cursors can still produce two EDITS whose ranges touch without their
/// selections doing so (two adjacent single-byte deletes), and
/// `Buffer::apply_edits` is deliberately permissive of a touching,
/// non-overlapping batch — see `apply_edits_descending_order_and_overlap`
/// in `buffer.rs`. The one real enforcement point is `apply_edits` itself:
/// `BufferError::DuplicateEditStart` refuses any batch whose computed
/// `AppliedEdit`s would collide, so any `edits` that came from a live
/// `apply_edits` call can never violate this precondition. What `reapply`
/// guards here is edits it did NOT produce itself — a persisted journal row
/// replayed from the recovery store, which predates that guard or was
/// written by a build that didn't enforce it — by refusing to replay them
/// rather than silently picking an order.
pub fn reapply(buf: &Buffer, edits: &[AppliedEdit]) -> Result<Buffer, BufferError> {
    if edits.is_empty() {
        return Ok(buf.clone());
    }
    let mut sorted: Vec<&AppliedEdit> = edits.iter().collect();
    sorted.sort_by_key(|e| e.start);
    if let Some(start) = duplicate_applied_start(edits) {
        return Err(BufferError::DuplicateEditStart { start });
    }

    let mut work = buf.clone();
    for e in sorted {
        let start = e.start;
        let end = match start.checked_add(e.deleted.len()) {
            Some(end) => end,
            None => {
                return Err(BufferError::OutOfBounds {
                    start,
                    end: usize::MAX,
                    len: work.len(),
                });
            }
        };
        if start > work.len() || end > work.len() || start > end {
            return Err(BufferError::OutOfBounds {
                start,
                end,
                len: work.len(),
            });
        }
        let edit = Edit {
            start,
            end,
            insert: e.insert.clone(),
        };
        let (new_buf, _) = work.apply_edits(std::slice::from_ref(&edit))?;
        work = new_buf;
    }
    Ok(work)
}

/// One undo/redo unit: the edits applied plus cursor state before/after, so
/// undo/redo restores selection alongside content. Shape implied by
/// `workspace_undo.go:31-85` (`step.Edits`, `step.Cursors`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Step {
    pub edits: Vec<AppliedEdit>,
    pub cursors_before: Vec<Cursor>,
    pub cursors_after: Vec<Cursor>,
}

/// In-memory undo/redo journal. `push` truncates any redo tail, matching a
/// normal editor undo stack (the durable store adds persistence behind the
/// same peek-then-commit shape, not a new one).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Journal {
    steps: Vec<Step>,
    pos: usize,
}

impl Journal {
    pub fn new() -> Journal {
        Journal {
            steps: Vec::new(),
            pos: 0,
        }
    }

    /// Record a new step, discarding any redo tail past the current
    /// position.
    pub fn push(&mut self, step: Step) {
        self.steps.truncate(self.pos);
        self.steps.push(step);
        self.pos = self.steps.len();
    }

    /// Peek the step undo would apply and the position undo would commit
    /// to — does NOT move `pos`. Callers apply the buffer edit first
    /// (`apply_inverse`) and call `move_pos` only on success (§1.4.8;
    /// `workspace_undo.go:31-46`).
    pub fn undo_peek(&self) -> Option<(&Step, usize)> {
        if self.pos == 0 {
            return None;
        }
        let new_pos = self.pos - 1;
        self.steps.get(new_pos).map(|step| (step, new_pos))
    }

    /// Peek the step redo would apply and the position redo would commit
    /// to — does NOT move `pos`. Mirrors `undo_peek`
    /// (`workspace_undo.go:88-` `handleRedo`).
    pub fn redo_peek(&self) -> Option<(&Step, usize)> {
        let step = self.steps.get(self.pos)?;
        Some((step, self.pos + 1))
    }

    /// Commit a journal position move. Call ONLY after the corresponding
    /// buffer edit (`apply_inverse`/`reapply`) has already succeeded — a
    /// failed apply must leave `pos` untouched so the journal never runs
    /// ahead of the buffer (§1.4.8).
    pub fn move_pos(&mut self, pos: usize) {
        self.pos = pos.min(self.steps.len());
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn undo_then_redo_round_trips_content() {
        let buf = Buffer::new("hello world");
        let (edited, applied) = buf
            .apply_edits(&[Edit {
                start: 5,
                end: 11,
                insert: " rust".to_string(),
            }])
            .expect("edit should apply");
        assert_eq!(edited.content(), "hello rust");

        let restored = apply_inverse(&edited, &applied).expect("inverse should apply");
        assert_eq!(restored.content(), "hello world");

        let redone = reapply(&restored, &applied).expect("reapply should apply");
        assert_eq!(redone.content(), "hello rust");
    }

    /// Documents the exact illegal-edit-set mechanism `apply_edits`'
    /// `DuplicateEditStart` rejection exists to catch, at the
    /// buffer-primitive level: two INDEPENDENT zero-width byte positions (0
    /// and 1 — not touching as cursor points, so `cursor::CursorSet::merge`
    /// correctly leaves them as two separate cursors) each deleting one
    /// byte forward derive ADJACENT, touching ranges `[0,1)` and `[1,2)`,
    /// both landing on post-edit `start == 0`: the identical illegal state
    /// `reapply` cannot safely order. `Buffer::apply_edits` refuses the
    /// batch outright now (see `apply_edits_rejects_a_batch_whose_edits_
    /// collide_on_post_edit_start` in `buffer.rs`) rather than handing the
    /// colliding `AppliedEdit`s to a caller — this test pins that the
    /// rejection fires for exactly the batch shape that used to slip
    /// through: a touching, non-overlapping pair that
    /// `coalesce_touching_edits` (in `rune-tui`, which this crate cannot see
    /// or depend on) would have merged before it ever reached here.
    #[test]
    fn adjacent_bare_deletes_collide_on_the_same_post_edit_start() {
        let buf = Buffer::new("ab");
        let err = buf.apply_edits(&[
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
        assert_eq!(
            err,
            Err(BufferError::DuplicateEditStart { start: 0 }),
            "two adjacent one-byte deletes would collapse to the identical \
             post-edit start — apply_edits must refuse the batch outright"
        );
    }

    /// The other half of the pin above, at `reapply`'s own boundary: a
    /// hand-built `AppliedEdit` batch with colliding starts — the shape a
    /// persisted journal row written before `apply_edits` enforced this
    /// invariant could still carry — is refused with the same
    /// `BufferError`, not replayed in whatever order the tied sort
    /// produces.
    #[test]
    fn reapply_refuses_a_batch_with_colliding_starts() {
        let buf = Buffer::new("ab");
        let applied = vec![
            AppliedEdit {
                start: 0,
                end: 0,
                deleted: "b".to_string(),
                insert: String::new(),
            },
            AppliedEdit {
                start: 0,
                end: 0,
                deleted: "a".to_string(),
                insert: String::new(),
            },
        ];
        let err = reapply(&buf, &applied);
        assert_eq!(err, Err(BufferError::DuplicateEditStart { start: 0 }));
    }

    #[test]
    fn journal_peek_does_not_move_position_until_committed() {
        let mut journal = Journal::new();
        journal.push(Step::default());
        assert_eq!(journal.pos(), 1);

        let (_, new_pos) = journal.undo_peek().expect("one step to undo");
        assert_eq!(journal.pos(), 1, "peek must not move pos");
        journal.move_pos(new_pos);
        assert_eq!(journal.pos(), 0);

        assert!(journal.undo_peek().is_none());
        let (_, redo_pos) = journal.redo_peek().expect("one step to redo");
        assert_eq!(journal.pos(), 0, "peek must not move pos");
        journal.move_pos(redo_pos);
        assert_eq!(journal.pos(), 1);
    }

    #[test]
    fn push_truncates_redo_tail() {
        let mut journal = Journal::new();
        journal.push(Step::default());
        journal.push(Step::default());
        journal.move_pos(1);
        journal.push(Step::default());
        assert_eq!(journal.len(), 2, "the discarded redo step must be gone");
        assert_eq!(journal.pos(), 2);
    }
}
