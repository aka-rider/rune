//! The shared buffer-mutation chokepoint underneath every editing command.
//! Both `edit`'s per-cursor commands and `edit_lines`'s line-oriented
//! commands route through the two functions here.
//!
//! `apply_edit_batch_with_cursors` is THE sole buffer-mutating primitive:
//! every command in `edit`/`edit_lines` funnels through it (directly, or
//! via `commit_edit_batch`'s generic per-cursor rule below) — one call,
//! one journal push, one undo step.

use rune_core::buffer::{AppliedEdit, Edit, SortedEdits};
use rune_core::cursor::{Cursor, CursorId, CursorSet};
use rune_core::undo::{EditKind, Step};

use crate::app::App;
use crate::db_enqueue as db;
use crate::document::DocumentId;
use crate::messages;
use crate::navhistory;

/// Shared low-level chokepoint underneath `commit_edit_batch`: sort the
/// batch, apply it, journal exactly ONE `Step`, mirror it to the async
/// replica, and recompute dirty — everything except how the post-edit
/// cursor SET is derived from the applied edits. Factored out (rather than
/// inlined in `commit_edit_batch`) so `edit_lines_move::move_line_up`/`down`
/// (which compute their own single resulting cursor — a column-preserving
/// placement WITHIN the moved line, not at the edit's end) can fund
/// through the exact same apply+journal-push code `commit_edit_batch`'s
/// generic per-cursor rule uses, rather than re-implementing (and risking
/// drifting from) it. `cursors_after` closes
/// over `applied`/`ids` in whatever way the caller's command needs; this
/// function does not know or care which rule that is — see
/// `commit_edit_batch` below for the generic one every other command uses.
///
/// Cursors only ever change on `Ok`; a rejected batch surfaces to the
/// status line and leaves buffer/cursors untouched — fail fast on data
/// risk, the same discipline `edit::undo`/`redo` already follow. Computing
/// a post-edit cursor set unconditionally, even when `apply_edits` itself
/// returns an error, would be a dead branch in practice (a cursor-derived
/// edit batch is always in-bounds), but not a Rust type-state worth
/// leaving standing.
///
/// Message OWNERSHIP: a successful edit posts nothing at all — the log is
/// append-only, so there is no shared slot to accidentally clear. This
/// function only ever POSTS on a refusal — the read-only rung below, or its
/// own failure path further down; an earlier unrelated entry (e.g. a save
/// failure) simply stays in the log.
///
/// The read-only CHOKEPOINT (review finding F1): every mutating command —
/// typing, backspace/delete, indent/outdent, cut, paste, move/clone/delete
/// line — funnels through this one function (via `commit_edit_batch` or
/// directly), so checking `app.doc(id).read_only` HERE (before anything
/// else — no partial work, no journal entry, no cursor change) makes "a
/// read-only document got mutated" unreachable regardless of which
/// command tried it, rather than relying on every call site to remember
/// its own guard (see `Document::read_only`'s docs for the bug this closes
/// and why `edit::undo`/`redo` are deliberately exempt). Checked via
/// `is_read_only()`, not the field directly — refusal must trigger on
/// every `ReadOnly` variant, not only `Always`. Posts `read_only`'s own
/// wording via `messages::warn_if_new`, so a held key against a read-only
/// document reports the reason once rather than flooding the log with an
/// identical line per keystroke.
///
/// Returns whether a batch actually applied (a journal `Step` was pushed) —
/// `false` for every refusal below (missing doc, read-only, empty-after-
/// retain) and for `Buffer::apply_edits`'s own rejection. The merge entry's
/// D3 invariant needs to tell "the working form actually landed in the
/// buffer" apart from "nothing happened" before it is safe to advance the
/// recovery store's CAS baseline (`resolve_adopt`) over that install — an
/// un-observable `()` return made that distinction impossible to make from
/// the call site.
pub(crate) fn apply_edit_batch_with_cursors(
    app: &mut App,
    id: DocumentId,
    mut infos: Vec<(Edit, CursorId)>,
    cursors_before: &CursorSet,
    kind: EditKind,
    cursors_after: impl FnOnce(&[AppliedEdit], &[CursorId]) -> Vec<Cursor>,
) -> bool {
    let Some(doc) = app.doc(id) else { return false };
    if doc.is_read_only() {
        if let Some(message) = doc.read_only.refusal_message() {
            messages::warn_if_new(app, message);
        }
        return false;
    }
    // A zero-width, insert-nothing edit (`start == end && insert.is_empty()`)
    // is a legal no-op at the buffer layer — `Buffer::apply_edits` accepts
    // it correctly, since nothing about the range or the insert is
    // ill-formed. But committing one here anyway would still push a `Step`
    // onto the journal and bump the buffer version for a batch that changed
    // nothing, marking a clean document dirty (e.g. cut on an empty
    // selection). This belongs at the single chokepoint every mutating
    // command already funnels through, not as a per-command spot check
    // duplicated at every call site.
    infos.retain(|(edit, _)| !(edit.start == edit.end && edit.insert.is_empty()));
    if infos.is_empty() {
        return false;
    }
    let mut infos = coalesce_touching_edits(infos);
    if let Some(start) = first_overlap_start(&infos) {
        messages::error(
            app,
            format!("edit failed: overlapping edits at byte {start}"),
        );
        return false;
    }
    infos.sort_by(|a, b| b.0.start.cmp(&a.0.start).then(b.0.end.cmp(&a.0.end)));

    let edits: Vec<Edit> = infos.iter().map(|(e, _)| e.clone()).collect();
    let ids: Vec<CursorId> = infos.iter().map(|(_, cid)| *cid).collect();

    match doc.buffer.apply_edits(&SortedEdits::sort(&edits)) {
        Ok((new_buf, applied)) => {
            let new_cursors = cursors_after(&applied, &ids);
            let Some(doc) = app.doc_mut(id) else {
                return false;
            };
            doc.buffer = new_buf;
            doc.cursors = CursorSet::new_from(&new_cursors);
            let cursors_after = doc.cursors.all().to_vec();
            let caret = doc.cursors.primary().position;
            doc.journal.push(Step {
                edits: applied.clone(),
                cursors_before: cursors_before.all().to_vec(),
                cursors_after: cursors_after.clone(),
                kind,
            });
            doc.ladder_presses = 0;
            doc.ladder_anchor = None;
            // The LOCAL journal above is already the authoritative,
            // synchronous source of truth — this enqueue can never roll it
            // back, only mark the store degraded on failure
            // (`db::append_edit`'s doc comment).
            db::append_edit(
                app,
                id,
                &applied,
                cursors_before.all(),
                &cursors_after,
                kind,
            );
            crate::merge::ranges::remap_after_edit_batch(app, id, &applied);
            for ae in applied.iter().rev() {
                app.nav_history
                    .shift(id, ae.start, ae.deleted.len(), ae.insert.len());
            }
            navhistory::record_edit(app, id, caret);
            true
        }
        Err(e) => {
            messages::error(app, format!("edit failed: {e}"));
            false
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
/// Two cursors landing on the byte-identical edit over a NON-ZERO-WIDTH
/// range (same `start != end`, same `insert` — e.g. two cursors inside
/// one word both extending to that word's range for a case change) are
/// deduped to one first, keeping the lower cursor id. A zero-width pure
/// insert is deliberately exempt from this dedup even when two cursors
/// produce byte-identical ones (see the clone-line paragraph below — each
/// insert is its own cursor's clone, not a duplicate of the other's).
/// What remains after dedup is coalesced only when the EARLIER edit in a
/// touching pair is a PURE DELETION (`insert.is_empty()`) — the only
/// other shape where two touching, non-identical ranges are genuinely the
/// same edit. Two cursors that legitimately share a line (clone-line's
/// `per_line_edits(dedupe=false)` keys edits on `line_start`, not on
/// selection, so this is reachable any time an edit joins two cursor-
/// bearing lines) each build their OWN distinct insert at the identical
/// point — `Buffer::apply_edits` gives each one a distinct post-edit
/// `start` (whichever insert the shift walk processes first lands before
/// the other's in the final text), so leaving them uncoalesced does not
/// collide; concatenating their inserts here instead would have silently
/// dropped one cursor's own edit (the bug this replaces — see
/// `edit_lines`'s clone-line-two-cursors-one-line test). A pure-deletion
/// pair has no such distinguishing content: `Buffer::apply_edits` would
/// hand both the SAME post-edit start (nothing inserted to separate
/// them), the exact illegal state `undo::reapply`'s precondition assert
/// exists to catch — coalescing those two ranges into one is the only
/// correct outcome. A remaining pair that still overlaps after both
/// passes is a genuine conflict, not a shape either pass is meant to
/// resolve — see `first_overlap_start` below.
///
/// Delegates the actual merge rule to `rune_core::undo::
/// coalesce_touching_deletes` — the same chokepoint `inverse_edits` uses
/// to fix up undo's own construction of a touching-pure-insert step's
/// inverse — rather than a second copy of the pure-delete merge condition.
/// The surviving cursor id is the lower of the two merged edits' ids,
/// matching this function's own doc above.
fn coalesce_touching_edits(infos: Vec<(Edit, CursorId)>) -> Vec<(Edit, CursorId)> {
    let deduped = dedupe_identical_edits(infos);
    rune_core::undo::coalesce_touching_deletes(deduped, CursorId::min)
}

fn dedupe_identical_edits(mut infos: Vec<(Edit, CursorId)>) -> Vec<(Edit, CursorId)> {
    infos.sort_by(|a, b| {
        a.0.start
            .cmp(&b.0.start)
            .then(a.0.end.cmp(&b.0.end))
            .then(a.0.insert.cmp(&b.0.insert))
    });
    let mut deduped: Vec<(Edit, CursorId)> = Vec::with_capacity(infos.len());
    for (edit, cid) in infos {
        let replaces_a_range = edit.start != edit.end;
        match deduped.last_mut() {
            Some(last) if replaces_a_range && last.0 == edit => last.1 = last.1.min(cid),
            _ => deduped.push((edit, cid)),
        }
    }
    deduped
}

fn first_overlap_start(infos: &[(Edit, CursorId)]) -> Option<usize> {
    infos.windows(2).find_map(|w| match w {
        [a, b] if a.0.end > b.0.start => Some(b.0.start),
        _ => None,
    })
}

pub(crate) fn commit_edit_batch(
    app: &mut App,
    id: DocumentId,
    infos: Vec<(Edit, CursorId)>,
    cursors_before: &CursorSet,
    kind: EditKind,
) -> bool {
    apply_edit_batch_with_cursors(app, id, infos, cursors_before, kind, |applied, ids| {
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
    })
}

#[cfg(test)]
#[path = "edit_core_tests.rs"]
mod tests;
