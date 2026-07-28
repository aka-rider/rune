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

/// Coalesces edits in `infos` whose `[start, end)` ranges touch or overlap
/// into one, before the batch ever reaches `Buffer::apply_edits`.
///
/// `CursorSet::merge` only ever coalesces cursors whose own selection
/// ranges touch — for a caretless cursor that is the zero-width point
/// `[position, position)`. Several commands' per-cursor edit generators
/// (Backspace/Delete/DeleteWord's `bare` closures in `commands::edit`, and
/// `commands::edit_lines`' outdent/delete-line) deliberately reach past
/// that point — one rune left, one rune right, one word, one whole line —
/// so two cursors close enough together can each still survive `merge`
/// (their own selection points never touch) while the EDITS their commands
/// produce do. `Buffer::apply_edits` accepts touching, non-overlapping
/// edits, but when the earlier one is a pure deletion its negative shift
/// can land the later edit's post-edit `start` on the exact same offset as
/// the earlier one's — the state `undo::reapply`'s invariant exists to
/// catch (redo would then replay the tied pair in an unspecified order).
///
/// Coalescing on the actual byte ranges here, rather than trusting the
/// cursor selections that produced them, makes that state unrepresentable
/// for every command that funnels through this chokepoint — not just the
/// one command that happened to surface it. Every edit whose range can
/// extend past its own cursor's selection in this codebase is a pure
/// deletion (`insert` empty): an inserting edit's `bare` range is always
/// the cursor's own zero-width point, and two distinct cursors — already
/// deduplicated by `merge` — can never share one. So the combined edit's
/// `insert` is simply the two, concatenated in ascending-`start` order:
/// empty + empty for every edit this actually coalesces today, and still
/// well-defined (never silently dropping or reordering text) should a
/// future command ever hand this chokepoint a non-empty one.
fn coalesce_touching_edits(infos: Vec<(Edit, u32)>) -> Vec<(Edit, u32)> {
    if infos.len() <= 1 {
        return infos;
    }
    let mut sorted = infos;
    sorted.sort_by(|a, b| a.0.start.cmp(&b.0.start).then(a.0.end.cmp(&b.0.end)));

    let mut iter = sorted.into_iter();
    let mut current = match iter.next() {
        Some(first) => first,
        None => return Vec::new(),
    };
    let mut out: Vec<(Edit, u32)> = Vec::with_capacity(iter.len());

    for next in iter {
        if current.0.end >= next.0.start {
            let start = current.0.start.min(next.0.start);
            let end = current.0.end.max(next.0.end);
            let mut insert = current.0.insert;
            insert.push_str(&next.0.insert);
            let id = current.1.min(next.1);
            current = (
                Edit {
                    start,
                    end,
                    insert,
                    cursor_id: 0,
                },
                id,
            );
        } else {
            out.push(current);
            current = next;
        }
    }
    out.push(current);
    out
}

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
    infos = coalesce_touching_edits(infos);
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::commands::edit;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    fn app_with(content: &str) -> App {
        let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
        let id = app.active;
        app.doc_mut(id)
            .expect("fixture doc must exist")
            .viewport
            .set_size(80, 23);
        app
    }

    #[test]
    fn coalesce_touching_edits_merges_two_adjacent_pure_deletions() {
        let infos = vec![
            (
                Edit {
                    start: 4,
                    end: 5,
                    insert: String::new(),
                    cursor_id: 0,
                },
                1,
            ),
            (
                Edit {
                    start: 3,
                    end: 4,
                    insert: String::new(),
                    cursor_id: 0,
                },
                2,
            ),
        ];
        let merged = coalesce_touching_edits(infos);
        assert_eq!(merged.len(), 1);
        assert_eq!(
            merged[0].0,
            Edit {
                start: 3,
                end: 5,
                insert: String::new(),
                cursor_id: 0
            }
        );
        assert_eq!(
            merged[0].1, 1,
            "lower id survives, mirroring CursorSet::merge's own tie-break"
        );
    }

    #[test]
    fn coalesce_touching_edits_leaves_non_touching_edits_apart() {
        let infos = vec![
            (
                Edit {
                    start: 0,
                    end: 1,
                    insert: String::new(),
                    cursor_id: 0,
                },
                1,
            ),
            (
                Edit {
                    start: 5,
                    end: 6,
                    insert: String::new(),
                    cursor_id: 0,
                },
                2,
            ),
        ];
        let merged = coalesce_touching_edits(infos);
        assert_eq!(merged.len(), 2);
    }

    /// The regression this fix exists for (root-caused via `make
    /// test-fuzz`'s `no-panic-c33c6055` artifact — see `repros/no-panic-01.
    /// rune` and the `TODO.md` entry it resolves): two cursors, one rune
    /// apart, neither with a selection, both Backspace. Neither cursor's
    /// own selection touches the other's, so `CursorSet::merge` correctly
    /// leaves them as two separate cursors — but Backspace's `bare` range
    /// reaches one rune LEFT of each cursor's position, so the two EDITS
    /// those cursors' commands produce DO touch (`[0,1)` and `[1,2)`).
    ///
    /// Without `coalesce_touching_edits` above, this batch would reach
    /// `Buffer::apply_edits` as two separate touching edits — this test's
    /// own `step.edits.len() == 1` assertion below catches that directly.
    /// Redoing that same two-edit `Step` is what actually trips
    /// `undo::reapply`'s `STRICT_INVARIANTS`-gated assertion in production
    /// (the earlier pure-deletion edit's negative shift collapses the
    /// later edit's post-edit `start` onto it) — not reproduced by THIS
    /// test, since this crate's own test build does not compile `rune-core`
    /// with `cfg(test)` or the `strict-invariants` feature; only the
    /// session fuzzer opts into that, deliberately. The checked-in replay
    /// repro is what proves the reapply panic itself is gone.
    /// Verified by temporarily reverting the `coalesce_touching_edits`
    /// call in `apply_edit_batch_with_cursors` and re-running this test: it
    /// then fails at the `step.edits.len()` assertion below.
    #[test]
    fn two_adjacent_cursors_backspacing_coalesce_into_one_edit_and_survive_redo() {
        let mut app = app_with("ab");
        let id = app.active;
        let doc = app.doc_mut(id).expect("fixture doc must exist");
        doc.cursors = CursorSet::new(1).add(Cursor {
            position: 2,
            anchor: 2,
            desired_col: 0,
            id: 0,
        });
        assert_eq!(
            doc.cursors.len(),
            2,
            "fixture must hold two cursors, one rune apart, for merge() to legitimately leave separate"
        );

        edit::delete_left(&mut app, id);
        assert_eq!(app.doc(id).expect("doc").buffer.content(), "");
        let step = app
            .doc(id)
            .expect("doc")
            .journal
            .undo_peek()
            .expect("one step to undo")
            .0;
        assert_eq!(
            step.edits.len(),
            1,
            "the two cursors' touching Backspace ranges must coalesce into one edit"
        );

        edit::undo(&mut app, id);
        assert_eq!(app.doc(id).expect("doc").buffer.content(), "ab");
        edit::redo(&mut app, id);
        assert_eq!(app.doc(id).expect("doc").buffer.content(), "");
    }
}
