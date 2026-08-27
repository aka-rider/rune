//! In-memory undo journal, SQLite-shaped for Phase 2 (the durable store adds
//! persistence behind the same peek-then-commit shape, not a new one).

use crate::buffer::{AppliedEdit, Buffer, BufferError, Edit, SortedEdits, duplicate_applied_start};
use crate::cursor::Cursor;

/// Merge every pair of adjacent/overlapping PURE-DELETE edits (`insert`
/// empty on BOTH) into one covering their union — the one shape
/// `Buffer::apply_edits`' `DuplicateEditStart` guard exists to refuse
/// downstream: two touching one-byte deletes are individually valid,
/// non-overlapping edits, but collapse to the identical post-edit `start`
/// once the earlier one's shift is accounted for. Merging first removes
/// the illegal state at its source instead of asking a caller to
/// disambiguate an already-collided pair. Chokepoint shared by
/// `inverse_edits` below (where two touching PURE-INSERT `AppliedEdit`s —
/// deliberately left un-merged going forward, in `rune-tui`'s own
/// `edit_core::coalesce_touching_edits`, so per-cursor identity survives a
/// clone-line-style batch — invert into two touching PURE DELETES, which
/// DO need merging: undo restores cursors from the step's own recorded
/// `cursors_before`, never from the inverse batch, so there is no cursor
/// identity left to lose here) and by that same `rune-tui` function, so
/// the merge rule itself is defined exactly once. `meta` rides along each
/// edit — a cursor id in `rune-tui`, `()` here — and `merge_meta` decides
/// how two merged edits' metadata combine.
pub fn coalesce_touching_deletes<T>(
    edits: Vec<(Edit, T)>,
    merge_meta: impl Fn(T, T) -> T,
) -> Vec<(Edit, T)> {
    if edits.len() <= 1 {
        return edits;
    }
    let mut sorted = edits;
    sorted.sort_by(|a, b| a.0.start.cmp(&b.0.start).then(a.0.end.cmp(&b.0.end)));

    let mut merged = Vec::with_capacity(sorted.len());
    let mut iter = sorted.into_iter();
    let Some(mut current) = iter.next() else {
        return merged;
    };
    for next in iter {
        let both_pure_deletes = current.0.insert.is_empty() && next.0.insert.is_empty();
        if both_pure_deletes && current.0.end >= next.0.start {
            let start = current.0.start.min(next.0.start);
            let end = current.0.end.max(next.0.end);
            current = (
                Edit {
                    start,
                    end,
                    insert: String::new(),
                },
                merge_meta(current.1, next.1),
            );
        } else {
            merged.push(current);
            current = next;
        }
    }
    merged.push(current);
    merged
}

/// Build the inverse edit batch for an applied-edit batch (undo): each
/// edit's insert becomes a delete range, and its deleted text becomes the
/// new insert.
///
/// Returns `BufferError::OutOfBounds` instead of panicking if
/// `ae.start + ae.insert.len()` would overflow `usize`. Unreachable from
/// any edit `apply_edits` itself produced (every real `start`/`len` is
/// bounded by a live document's byte length), but `edits` is `pub fn`
/// -reachable data — Phase 2 will feed it back from SQLite — and
/// adversarial input must never panic the process, however unreachable
/// that path is today.
///
/// Runs `coalesce_touching_deletes` on the raw inverse batch before
/// sorting/returning it: a forward step recording two touching PURE
/// INSERTS (legitimate — e.g. two multicursor `clone-line` edits on
/// adjacent lines, each landing at its own distinct post-edit `start`, so
/// the forward apply never collides) inverts into two touching PURE
/// DELETES, which — left separate — WOULD collide on `apply_edits`'
/// `DuplicateEditStart` check the moment this batch is applied. Merging
/// here means undo is total for exactly the batches the forward path
/// legitimately allowed to be recorded, without weakening
/// `DuplicateEditStart` itself: any batch that still collides after this
/// merge (e.g. a corrupted/adversarial persisted journal row) is still
/// refused by `apply_edits`, unchanged.
pub fn inverse_edits(edits: &[AppliedEdit]) -> Result<SortedEdits, BufferError> {
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
        raw.push((
            Edit {
                start: ae.start,
                end,
                insert: ae.deleted.clone(),
            },
            (),
        ));
    }
    let merged = coalesce_touching_deletes(raw, |(), ()| ());
    let plain: Vec<Edit> = merged.into_iter().map(|(e, ())| e).collect();
    Ok(SortedEdits::sort(&plain))
}

/// Apply the inverse of `edits` to `buf` (undo). All-or-nothing: on error
/// `buf` is untouched by the caller — the journal position must stay put
/// too.
pub fn apply_inverse(buf: &Buffer, edits: &[AppliedEdit]) -> Result<Buffer, BufferError> {
    let inverse = inverse_edits(edits)?;
    let (new_buf, _) = buf.apply_edits(&inverse)?;
    Ok(new_buf)
}

/// `reapply`'s own single-edit application boundary — [`SortedEdits::validate`]
/// rather than [`SortedEdits::sort`], since a persisted journal row replayed
/// through this function (`rune-db`'s recovery-store snapshot rebuild) is
/// adversarial, decoded input: a single edit is always trivially ordered,
/// but proving that rather than assuming it keeps this call site honest if
/// `reapply` is ever changed to batch more than one edit at a time.
fn apply_one_validated(buf: &Buffer, edit: Edit) -> Result<Buffer, BufferError> {
    let sorted = SortedEdits::validate(vec![edit])?;
    let (new_buf, _) = buf.apply_edits(&sorted)?;
    Ok(new_buf)
}

/// Reapply `edits` forward against `buf` (redo), one edit at a time,
/// ascending by `start`, against a running copy. `AppliedEdit::start`
/// carries a baked-in cumulative shift that is only valid one edit at a
/// time against the running buffer. All-or-nothing: any edit's failure
/// returns the error and the original `buf` is never touched.
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
        work = apply_one_validated(&work, edit)?;
    }
    Ok(work)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EditKind {
    Insert,
    DeleteLeft,
    DeleteRight,
    Paste,
    Cut,
    StripTrailingWhitespace,
    #[default]
    Other,
}

/// One undo/redo unit: the edits applied plus cursor state before/after, so
/// undo/redo restores selection alongside content.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Step {
    pub edits: Vec<AppliedEdit>,
    pub cursors_before: Vec<Cursor>,
    pub cursors_after: Vec<Cursor>,
    pub kind: EditKind,
}

#[derive(Debug, Clone, Copy)]
pub struct JournalCommit(usize);

impl JournalCommit {
    pub fn target(&self) -> usize {
        self.0
    }
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

    pub fn undo_peek(&self) -> Option<(&Step, JournalCommit)> {
        if self.pos == 0 {
            return None;
        }
        let new_pos = self.pos - 1;
        self.steps
            .get(new_pos)
            .map(|step| (step, JournalCommit(new_pos)))
    }

    pub fn redo_peek(&self) -> Option<(&Step, JournalCommit)> {
        let step = self.steps.get(self.pos)?;
        Some((step, JournalCommit(self.pos + 1)))
    }

    pub fn commit(&mut self, token: JournalCommit) {
        self.pos = token.0;
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

#[cfg(test)]
#[path = "undo_tests.rs"]
mod tests;
