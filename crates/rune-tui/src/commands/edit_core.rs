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
    mut infos: Vec<(Edit, u32)>,
    cursors_before: CursorSet,
    cursors_after: impl FnOnce(&[AppliedEdit], &[u32]) -> Vec<Cursor>,
) {
    let Some(doc) = app.doc(id) else { return };
    if doc.read_only || infos.is_empty() {
        return;
    }
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
