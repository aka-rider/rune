//! The shared buffer-mutation chokepoint underneath every editing command
//! (plan WP9.S6 §1.6 split — extracted from `edit.rs` alongside the
//! `edit_lines` split, since `edit.rs` was still over budget with just
//! that one boundary; both `edit`'s per-cursor commands and `edit_lines`'
//! line-oriented commands route through the two functions here, so this
//! is genuinely their shared home, not an arbitrary third bucket).
//!
//! `apply_edit_batch_with_cursors` is THE sole buffer-mutating primitive:
//! every command in `edit`/`edit_lines` funnels through it (directly, or
//! via `commit_edit_batch`'s generic per-cursor rule below) — one call,
//! one journal push, one undo step. Port of
//! `commands_edit_lines.go:sortInfosDescending` + `buildEditResultFromInfos`
//! + `textedit.go:applyOperation`'s edit-apply branch + `commitEdits`.

use rune_core::buffer::{AppliedEdit, Edit};
use rune_core::cursor::{Cursor, CursorSet};
use rune_core::undo::Step;

use crate::app::{App, StatusSource};
use crate::db;
use crate::document::DocumentId;
use crate::save;

/// Shared low-level chokepoint underneath `commit_edit_batch`: sort the
/// batch, apply it, journal exactly ONE `Step`, mirror it to the async
/// replica, and recompute dirty — everything except how the post-edit
/// cursor SET is derived from the applied edits. Factored out (rather than
/// inlined in `commit_edit_batch`) so `edit_lines::move_line_up`/`down`
/// (Go's `execMoveLineUp`/`execMoveLineDown`, which are NOT built on
/// `buildEditResultFromInfos` and compute their own single resulting
/// cursor — a column-preserving placement WITHIN the moved line, not at
/// the edit's end) can fund through the exact same apply+journal-push code
/// `commit_edit_batch`'s generic per-cursor rule uses, rather than
/// re-implementing (and risking drifting from) it. `cursors_after` closes
/// over `applied`/`ids` in whatever way the caller's command needs; this
/// function does not know or care which rule that is — see
/// `commit_edit_batch` below for the generic one every other command uses.
///
/// Deliberate improvement over Go's `applyOperation`: Go assigns
/// `result.Operation.Cursors` (computed as if the edit succeeded)
/// UNCONDITIONALLY, even when `ApplyEdits` itself returned an error — a
/// dead branch in practice (a cursor-derived edit batch is always
/// in-bounds), but not a Rust type-state to leave standing. Here cursors
/// only ever change on `Ok`; a rejected batch surfaces to the status line
/// and leaves buffer/cursors untouched (CONSTITUTION §1.3: "fail fast on
/// data risk", the same discipline `edit::undo`/`redo` already follow).
///
/// Status-message OWNERSHIP (review finding F2): a successful edit must
/// NOT clear `app.status_message` — that field is a shared, single slot
/// with no provenance tag, so an unconditional clear here would erase an
/// unrelated message another subsystem is still showing (the reported
/// case: a save failure, "save failed: disk full", vanishing on the very
/// next keystroke while the buffer is still dirty — the user's only
/// signal, gone). This function only ever WRITES `status_message` on its
/// own failure path below; a message set by someone else survives here
/// until whatever set it (or a later, unrelated event) supersedes it.
///
/// The read-only CHOKEPOINT (review finding F1): every mutating command —
/// typing, backspace/delete, indent/outdent, cut, paste, move/clone/delete
/// line — funnels through this one function (via `commit_edit_batch` or
/// directly), so checking `app.doc(id).read_only` HERE (before anything
/// else — no partial work, no journal entry, no cursor change) makes "a
/// read-only document got mutated" unreachable regardless of which
/// command tried it, rather than relying on every call site to remember
/// its own guard (see `Document::read_only`'s docs for the bug this closes
/// and why `edit::undo`/`redo` are deliberately exempt).
pub(crate) fn apply_edit_batch_with_cursors(
    app: &mut App,
    id: DocumentId,
    infos: Vec<(Edit, u32)>,
    cursors_before: CursorSet,
    cursors_after: impl FnOnce(&[AppliedEdit], &[u32]) -> Vec<Cursor>,
) {
    let Some(doc) = app.doc(id) else { return };
    if doc.read_only || infos.is_empty() {
        return;
    }
    let mut infos = coalesce_touching_edits(infos);
    infos.sort_by(|a, b| b.0.start.cmp(&a.0.start).then(b.0.end.cmp(&a.0.end)));

    let edits: Vec<Edit> = infos.iter().map(|(e, _)| e.clone()).collect();
    let ids: Vec<u32> = infos.iter().map(|(_, cid)| *cid).collect();

    match doc.buffer.apply_edits(&edits) {
        Ok((new_buf, applied)) => {
            let new_cursors = cursors_after(&applied, &ids);
            let Some(doc) = app.doc_mut(id) else { return };
            doc.buffer = new_buf;
            doc.cursors = CursorSet::new_from(&new_cursors);
            let cursors_after = doc.cursors.all();
            doc.journal.push(Step {
                edits: applied.clone(),
                cursors_before: cursors_before.all(),
                cursors_after: cursors_after.clone(),
            });
            // Async replica journaling (plan WP5.S3): the LOCAL journal
            // above is already the authoritative, synchronous source of
            // truth — this enqueue can never roll it back, only mark the
            // store degraded on failure (`db::append_edit`'s doc comment).
            let local_pos = doc.journal.pos();
            db::append_edit(
                app,
                id,
                local_pos,
                &applied,
                &cursors_before.all(),
                &cursors_after,
            );
            save::recompute_dirty(app, id);
        }
        Err(e) => {
            app.set_status(format!("edit failed: {e}"), StatusSource::Other);
        }
    }
}

/// Coalesces any two edits in the batch whose PRE-edit ranges touch or
/// overlap into one, unioning the range and keeping the lower cursor id as
/// survivor — the edit-CONSTRUCTION-level analogue of `CursorSet::merge`,
/// and this function's real invariant-preserving chokepoint (every batch
/// passes through here before it ever reaches `Buffer::apply_edits`).
///
/// `CursorSet::merge` only ever sees raw cursor positions/selections, and
/// correctly leaves two cursors separate whenever those don't touch. But a
/// per-cursor command (delete-right, delete-word-left/right, ...) then
/// derives a byte RANGE from each cursor's position — extended forward or
/// backward by a rune or a word — and two such derived ranges from
/// perfectly legitimate, non-touching cursors can still end up touching or
/// overlapping (e.g. cursor A at byte 0 and cursor B at byte 1, both
/// pressing Delete: A's range is `[0,1)`, B's is `[1,2)`). `CursorSet::
/// merge` has no visibility into that derived range, so it cannot catch
/// this — the edit-building step is the only place that can, since it is
/// the only place both ranges exist at once.
///
/// Left unmerged, a touching pair reaches `Buffer::apply_edits` as two
/// "non-overlapping" edits that individually pass validation, but whose
/// post-edit `AppliedEdit::start` collapse to the identical offset once
/// the shift from applying the earlier one is accounted for — the exact
/// illegal state `undo::reapply`'s precondition assert exists to catch. An
/// overlapping (not just touching) pair is rejected outright by
/// `Buffer::apply_edits` as `EditsNotSortedOrOverlapping`, surfacing a
/// spurious "edit failed" to the user for an entirely ordinary multi-
/// cursor action. Coalescing here removes both illegal states at their
/// source instead of guarding against them downstream: two touching or
/// overlapping ranges really are one edit over their union.
///
/// Every real caller's colliding ranges carry an empty `insert` (all of
/// `delete_left`/`delete_right`/`delete_word_left`/`delete_word_right`'s
/// bare closures are pure deletions) — `HasSelection()`-replacement edits
/// can never collide, because `CursorSet::merge` already coalesces any two
/// cursors whose SELECTIONS touch before their edits are ever built.
/// Concatenating `insert` in range order keeps this correct even so.
fn coalesce_touching_edits(infos: Vec<(Edit, u32)>) -> Vec<(Edit, u32)> {
    if infos.len() <= 1 {
        return infos;
    }
    let mut sorted = infos;
    sorted.sort_by(|a, b| a.0.start.cmp(&b.0.start).then(a.0.end.cmp(&b.0.end)));

    let mut merged: Vec<(Edit, u32)> = Vec::with_capacity(sorted.len());
    let mut iter = sorted.into_iter();
    let Some(mut current) = iter.next() else {
        return merged;
    };
    for next in iter {
        if current.0.end >= next.0.start {
            let start = current.0.start.min(next.0.start);
            let end = current.0.end.max(next.0.end);
            let mut insert = current.0.insert;
            insert.push_str(&next.0.insert);
            let cursor_id = current.1.min(next.1);
            current = (
                Edit {
                    start,
                    end,
                    insert,
                    cursor_id,
                },
                cursor_id,
            );
        } else {
            merged.push(current);
            current = next;
        }
    }
    merged.push(current);
    merged
}

/// The generic per-cursor rule every command except `edit_lines::
/// move_line_up`/`down` uses: each surviving cursor lands at its own
/// edit's `AppliedEdit::end` (`start + insert.len()`, already in POST-edit
/// coordinates per `buffer.rs`'s own docs) — using it directly is simpler
/// than re-deriving Go's `computePostEditCursors` shift accumulation and
/// can never disagree with what `Buffer::apply_edits` actually did, since
/// it comes from the same call. `pub(crate)` so both `edit`'s per-cursor
/// commands and `edit_lines::per_line_edits` (indent/outdent/delete-line/
/// clone-line, whose post-edit cursor also lands at each edit's own end —
/// see that module's doc comment) share it rather than re-implementing
/// the same rule twice.
pub(crate) fn commit_edit_batch(
    app: &mut App,
    id: DocumentId,
    infos: Vec<(Edit, u32)>,
    cursors_before: CursorSet,
) {
    apply_edit_batch_with_cursors(app, id, infos, cursors_before, |applied, ids| {
        applied
            .iter()
            .zip(ids.iter())
            .map(|(ae, &cid)| Cursor {
                position: ae.end,
                anchor: ae.end,
                desired_col: 0,
                id: cid,
            })
            .collect()
    });
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// Two independent, non-touching CURSORS — `CursorSet::merge` correctly
    /// leaves positions 0 and 1 separate, since neither's (zero-width)
    /// selection touches the other's — each derive a one-byte Delete-
    /// forward range from their own position: `[0,1)` and `[1,2)`. Those
    /// two DERIVED ranges do touch. Left unmerged, `Buffer::apply_edits`
    /// hands back two `AppliedEdit`s that both land on post-edit `start ==
    /// 0` — the exact illegal state `undo::reapply`'s precondition assert
    /// exists to catch (`crates/rune-fuzz` artifact `no-panic-7f29861c`,
    /// checked in as `repros/no-panic-01.rune`).
    #[test]
    fn merges_two_adjacent_bare_deletes() {
        let infos = vec![
            (
                Edit {
                    start: 1,
                    end: 2,
                    insert: String::new(),
                    cursor_id: 2,
                },
                2,
            ),
            (
                Edit {
                    start: 0,
                    end: 1,
                    insert: String::new(),
                    cursor_id: 1,
                },
                1,
            ),
        ];
        let merged = coalesce_touching_edits(infos);
        assert_eq!(
            merged.len(),
            1,
            "touching ranges must collapse into one edit"
        );
        assert_eq!(
            merged.first(),
            Some(&(
                Edit {
                    start: 0,
                    end: 2,
                    insert: String::new(),
                    cursor_id: 1,
                },
                1,
            )),
            "the lower cursor id survives, matching CursorSet::merge's own rule"
        );
    }

    /// Two cursors sitting inside the same word: a delete-word-right from
    /// each derives OVERLAPPING (not just touching) ranges `[0,5)` and
    /// `[2,7)`. `Buffer::apply_edits` would otherwise reject this batch
    /// outright as `EditsNotSortedOrOverlapping` — a spurious "edit
    /// failed" for an entirely ordinary multi-cursor action.
    #[test]
    fn merges_overlapping_word_deletes() {
        let infos = vec![
            (
                Edit {
                    start: 2,
                    end: 7,
                    insert: String::new(),
                    cursor_id: 9,
                },
                9,
            ),
            (
                Edit {
                    start: 0,
                    end: 5,
                    insert: String::new(),
                    cursor_id: 3,
                },
                3,
            ),
        ];
        let merged = coalesce_touching_edits(infos);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged.first().map(|(e, _)| (e.start, e.end)), Some((0, 7)));
    }

    /// A real gap between two cursors' ranges must survive untouched —
    /// the common case (most multi-cursor edits do not collide at all).
    #[test]
    fn leaves_genuinely_separated_edits_alone() {
        let infos = vec![
            (
                Edit {
                    start: 5,
                    end: 6,
                    insert: String::new(),
                    cursor_id: 2,
                },
                2,
            ),
            (
                Edit {
                    start: 0,
                    end: 1,
                    insert: String::new(),
                    cursor_id: 1,
                },
                1,
            ),
        ];
        let merged = coalesce_touching_edits(infos);
        assert_eq!(merged.len(), 2, "a real gap must not be merged away");
    }
}
