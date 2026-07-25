//! In-memory undo journal, SQLite-shaped for Phase 2 (the durable store adds
//! persistence behind the same peek-then-commit shape, not a new one).
//! Port of the inverse/reapply formulas in
//! `pkg/ui/components/textedit/edit_primitives.go:51-110` and the
//! peek-then-commit journal discipline in
//! `pkg/ui/pages/workspace/workspace_undo.go:31-85`.

use crate::buffer::{AppliedEdit, Buffer, BufferError, Edit, clone_and_sort_edits_descending};
use crate::cursor::Cursor;

/// Build the inverse edit batch for an applied-edit batch (undo): each
/// edit's insert becomes a delete range, and its deleted text becomes the
/// new insert. Port of the edit construction in
/// `edit_primitives.go:51-60` (`ApplyInverse`).
pub fn inverse_edits(edits: &[AppliedEdit]) -> Vec<Edit> {
    let raw: Vec<Edit> = edits
        .iter()
        .map(|ae| Edit {
            start: ae.start,
            end: ae.start + ae.insert.len(),
            insert: ae.deleted.clone(),
            cursor_id: 0,
        })
        .collect();
    clone_and_sort_edits_descending(&raw)
}

/// Apply the inverse of `edits` to `buf` (undo). All-or-nothing: on error
/// `buf` is untouched by the caller — the journal position must stay put
/// too (§1.4.8, `workspace_undo.go:31-46`). Port of
/// `edit_primitives.go:51-68`.
pub fn apply_inverse(buf: &Buffer, edits: &[AppliedEdit]) -> Result<Buffer, BufferError> {
    let inverse = inverse_edits(edits);
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
pub fn reapply(buf: &Buffer, edits: &[AppliedEdit]) -> Result<Buffer, BufferError> {
    if edits.is_empty() {
        return Ok(buf.clone());
    }
    let mut sorted: Vec<&AppliedEdit> = edits.iter().collect();
    sorted.sort_by_key(|e| e.start);

    let mut work = buf.clone();
    for e in sorted {
        let start = e.start;
        let end = e.start + e.deleted.len();
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
            cursor_id: 0,
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
/// normal editor undo stack (Phase 2 gives this the same shape backed by
/// SQLite — see plan Context, "Undo journal").
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
                cursor_id: 0,
            }])
            .expect("edit should apply");
        assert_eq!(edited.content(), "hello rust");

        let restored = apply_inverse(&edited, &applied).expect("inverse should apply");
        assert_eq!(restored.content(), "hello world");

        let redone = reapply(&restored, &applied).expect("reapply should apply");
        assert_eq!(redone.content(), "hello rust");
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
