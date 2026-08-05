//! Editing (insert/backspace/delete/delete-word/newline) and undo/redo
//! (WP7): per-cursor edit commands run through a shared driver
//! (`per_cursor_selection_edits`), and undo/redo follow a peek-then-commit
//! discipline (see `undo`/`redo` below). The shared buffer-mutation
//! chokepoint (`apply_edit_batch_with_cursors`/`commit_edit_batch`) lives
//! in the sibling `edit_core` module, and the line-oriented commands
//! (indent/outdent, delete-line, clone/move-line) live in the sibling
//! `edit_lines` module (plan WP9.S6 500-line budget — one
//! `edit_lines` boundary was not enough to bring this file itself under
//! budget, hence the second split into `edit_core`) — see each module's
//! own doc for why the boundary sits there.
//!
//! Workspace-coupled (plan WP1 decision 4): every function here takes
//! `(app: &mut App, id: DocumentId)` — every mutation funnels through
//! `edit_core::commit_edit_batch`, which also touches `app.db`/
//! `app.status_message`/the dirty cache, so unlike `commands::nav` this
//! module can't work off a bare `&mut Document`. Internally, functions
//! borrow `app.doc_mut(id)` SEQUENTIALLY — mutate the doc, let that borrow
//! end, then call `db::append_edit(app, id, ...)`/`materialize_ack::recompute_dirty(
//! app, id)` — never a split-borrow context type.
//!
//! Backspace/delete-right are RUNE-aware, not grapheme-cluster-aware: the
//! offset walk decodes one UTF-8 codepoint at a time, with no
//! grapheme-cluster segmentation in the delete path. `Grapheme` names
//! appear elsewhere in this codebase (a per-cell rendered glyph string,
//! the image renderer, a fuzz artifact's serialized snapshot) but those
//! are RENDER-TIME display-cell payloads (what glyph a cell shows), never
//! consulted by the delete path's offset computation. A ZWJ emoji family
//! sequence therefore deletes one codepoint per Backspace, not the whole
//! cluster — a deliberate choice, not an oversight.

use rune_core::buffer::{AppliedEdit, Buffer, Edit};
use rune_core::cursor::{Cursor, CursorSet};

use crate::app::{App, StatusSource};
use crate::commands::edit_core::commit_edit_batch;
use crate::commands::nav;
use crate::commands::nav_line;
use crate::db_enqueue as db;
use crate::document::{DocumentId, ReadOnly};
use crate::materialize_ack;

/// One edit per cursor, replacing its selection when it has one, or
/// `bare`'s caller-chosen range otherwise. `bare` returning `None` skips
/// that cursor entirely (e.g. Backspace at buffer start).
fn per_cursor_selection_edits(
    app: &mut App,
    id: DocumentId,
    text_for: impl Fn(usize, &Cursor, &Buffer) -> String,
    bare: impl Fn(&Buffer, &Cursor) -> Option<(usize, usize)>,
) {
    let Some(doc) = app.doc(id) else { return };
    let cursors_before = doc.cursors.clone();
    let all = cursors_before.all();
    if all.is_empty() {
        return;
    }

    let mut infos: Vec<(Edit, u32)> = Vec::new();
    for (i, c) in all.iter().enumerate() {
        let Some(doc) = app.doc(id) else { return };
        let buf = &doc.buffer;
        let edit = if c.has_selection() {
            let start = c.selection_start();
            let end = nav::selection_end_inclusive(c, buf);
            Edit {
                start,
                end,
                insert: text_for(i, c, buf),
            }
        } else if let Some((start, end)) = bare(buf, c) {
            Edit {
                start,
                end,
                insert: text_for(i, c, buf),
            }
        } else {
            continue;
        };
        infos.push((edit, c.id));
    }

    let _ = commit_edit_batch(app, id, infos, cursors_before);
}

/// Generalized to arbitrary text so it doubles as the selection-replacing
/// insert path for bracketed paste (`Msg::Paste`, plan Context:
/// "Bracketed-paste `Msg::Paste` may insert text through the same insert
/// path").
pub fn insert_text(app: &mut App, id: DocumentId, text: &str) {
    if text.is_empty() {
        return;
    }
    per_cursor_selection_edits(
        app,
        id,
        move |_i, _c, _buf| text.to_string(),
        |_buf, c| Some((c.position, c.position)),
    );
}

pub fn insert_char(app: &mut App, id: DocumentId, ch: char) {
    let mut buf = [0u8; 4];
    insert_text(app, id, ch.encode_utf8(&mut buf));
}

/// The Enter hardcoded fast path (plan Context, "Hardcoded fast paths
/// outside the resolver"): inserts a newline plus the CURRENT line's own
/// leading whitespace, preserving indentation.
pub fn newline(app: &mut App, id: DocumentId) {
    per_cursor_selection_edits(
        app,
        id,
        |_i, c, buf| {
            let pos = if c.has_selection() {
                c.selection_start()
            } else {
                c.position
            };
            let bp = buf.offset_to_line_col(pos);
            let line = buf.line(bp.line);
            let indent: String = line
                .chars()
                .take_while(|&ch| ch == ' ' || ch == '\t')
                .collect();
            format!("\n{indent}")
        },
        |_buf, c| Some((c.position, c.position)),
    );
}

/// Reused by `commands::clipboard::cut` (WP8): deletes each cursor's
/// selection, or — with no selection — its whole current line including
/// the trailing `\n` (`nav_line::line_range_incl_newline`, the same range
/// `copy_entire_line` used to build the text cut just copied — so cut
/// always removes precisely what it captured).
pub(crate) fn delete_selection_or_line(app: &mut App, id: DocumentId) {
    per_cursor_selection_edits(
        app,
        id,
        |_i, _c, _buf| String::new(),
        |buf, c| Some(nav_line::line_range_incl_newline(buf, c.position)),
    );
}

pub fn delete_left(app: &mut App, id: DocumentId) {
    per_cursor_selection_edits(
        app,
        id,
        |_i, _c, _buf| String::new(),
        |buf, c| {
            if c.position == 0 {
                None
            } else {
                Some((nav::prev_rune_offset(buf, c.position), c.position))
            }
        },
    );
}

pub fn delete_right(app: &mut App, id: DocumentId) {
    per_cursor_selection_edits(
        app,
        id,
        |_i, _c, _buf| String::new(),
        |buf, c| {
            if c.position >= buf.len() {
                None
            } else {
                Some((c.position, nav::next_rune_offset(buf, c.position)))
            }
        },
    );
}

/// (plan WP9.S2) Deletes the selection when the cursor has one (same
/// selection-first rule every `per_cursor_selection_edits` caller
/// shares); otherwise deletes from `nav::word_left_offset` up to the
/// caret — one word, not one rune.
pub fn delete_word_left(app: &mut App, id: DocumentId) {
    per_cursor_selection_edits(
        app,
        id,
        |_i, _c, _buf| String::new(),
        |buf, c| {
            if c.position == 0 {
                None
            } else {
                Some((nav::word_left_offset(buf, c.position), c.position))
            }
        },
    );
}

pub fn delete_word_right(app: &mut App, id: DocumentId) {
    per_cursor_selection_edits(
        app,
        id,
        |_i, _c, _buf| String::new(),
        |buf, c| {
            if c.position >= buf.len() {
                None
            } else {
                Some((c.position, nav::word_right_offset(buf, c.position)))
            }
        },
    );
}

/// Peek the target step (without moving the journal), apply its inverse
/// to the buffer, and commit the position move ONLY if the buffer edit
/// succeeds — a failed apply surfaces a status-line error and leaves the
/// journal position (and buffer) untouched, so the journal never runs
/// ahead of the buffer. Same status-message ownership rule as
/// `commit_edit_batch` (F2): success never clears `app.status_message` —
/// only this function's own failure path writes it.
///
/// Gated on `ReadOnly::Reading`/`ReadOnly::Preview` only, not
/// `is_read_only()` — see `Document::read_only`'s doc comment for why
/// `ReadOnly::Always` stays exempt. `ReadOnly::Preview` routes through
/// `App::refuse_if_preview` so the refusal posts a status message like
/// every other preview refusal; `ReadOnly::Reading`'s refusal stays silent,
/// unchanged from before.
pub fn undo(app: &mut App, id: DocumentId) {
    if app.refuse_if_preview(id) {
        return;
    }
    let Some(doc) = app.doc(id) else { return };
    if matches!(doc.read_only, ReadOnly::Reading) {
        return;
    }
    let Some((step, new_pos)) = doc.journal.undo_peek() else {
        return;
    };
    let edits: Vec<AppliedEdit> = step.edits.clone();
    let cursors_before: Vec<Cursor> = step.cursors_before.clone();

    // The step's own `AppliedEdit`s are already in the CURRENT (pre-undo)
    // buffer's coordinates (`buffer.rs`: `AppliedEdit::start`/`end` is the
    // post-edit range of the edit that produced the buffer as it stands
    // right now) — exactly the span undo is about to overwrite (plan review
    // F1).
    let affected = affected_range(&edits);

    match rune_core::undo::apply_inverse(&doc.buffer, &edits) {
        Ok(new_buf) => {
            let Some(doc) = app.doc_mut(id) else { return };
            doc.buffer = new_buf;
            doc.cursors = CursorSet::new_from(&cursors_before);
            doc.journal.move_pos(new_pos);
            db::move_undo_pos(app, id, new_pos);
            materialize_ack::recompute_dirty(app, id);
            resync_after_journal_jump(app, id, affected);
        }
        Err(e) => {
            app.set_status(format!("undo failed: {e}"), StatusSource::Other);
        }
    }
}

/// Mirrors `undo` above: reapply the step forward, commit the position
/// move only on success. Same status-message ownership rule as
/// `commit_edit_batch`/`undo` (F2).
///
/// Gated on `ReadOnly::Reading`/`ReadOnly::Preview` only, not
/// `is_read_only()` — see `Document::read_only`'s doc comment for why
/// `ReadOnly::Always` stays exempt. `ReadOnly::Preview` routes through
/// `App::refuse_if_preview` so the refusal posts a status message like
/// every other preview refusal; `ReadOnly::Reading`'s refusal stays silent,
/// unchanged from before.
pub fn redo(app: &mut App, id: DocumentId) {
    if app.refuse_if_preview(id) {
        return;
    }
    let Some(doc) = app.doc(id) else { return };
    if matches!(doc.read_only, ReadOnly::Reading) {
        return;
    }
    let Some((step, new_pos)) = doc.journal.redo_peek() else {
        return;
    };
    let edits: Vec<AppliedEdit> = step.edits.clone();
    let cursors_after: Vec<Cursor> = step.cursors_after.clone();

    // Redo replays each edit's DELETE range — `[start, start + deleted.len())`
    // — against the running CURRENT (pre-redo) buffer (`rune_core::undo::
    // reapply`'s own doc), which is the same buffer state the step's
    // original forward application saw (no other edits interpose between an
    // undo and its matching redo) — so this is that same pre-redo buffer's
    // coordinates too (plan review F1).
    let affected = affected_delete_range(&edits);

    match rune_core::undo::reapply(&doc.buffer, &edits) {
        Ok(new_buf) => {
            let Some(doc) = app.doc_mut(id) else { return };
            doc.buffer = new_buf;
            doc.cursors = CursorSet::new_from(&cursors_after);
            doc.journal.move_pos(new_pos);
            db::move_undo_pos(app, id, new_pos);
            materialize_ack::recompute_dirty(app, id);
            resync_after_journal_jump(app, id, affected);
        }
        Err(e) => {
            app.set_status(format!("redo failed: {e}"), StatusSource::Other);
        }
    }
}

/// The union of every edit's POST-edit `[start, end)` span (plan review
/// F1) — the CURRENT buffer's own touched range immediately before an
/// undo overwrites it. `None` for an empty step (never reachable through a
/// real journal push, but total rather than assuming).
fn affected_range(edits: &[AppliedEdit]) -> Option<std::ops::Range<usize>> {
    let start = edits.iter().map(|e| e.start).min()?;
    let end = edits.iter().map(|e| e.end).max()?;
    Some(start..end)
}

/// The union of every edit's PRE-edit delete range — `[start, start +
/// deleted.len())`, the same range `rune_core::undo::reapply` computes
/// internally per edit (plan review F1). `None` for an empty step.
fn affected_delete_range(edits: &[AppliedEdit]) -> Option<std::ops::Range<usize>> {
    let start = edits.iter().map(|e| e.start).min()?;
    let end = edits.iter().map(|e| e.start + e.deleted.len()).max()?;
    Some(start..end)
}

/// Plan WP6.S1/S2: every undo/redo that actually applied re-derives the
/// resolver's block spans (`merge::resync`) when `id`'s merge attempt is
/// `Active` — a journal jump bypasses the resolver's own keys entirely, so
/// without this its `Block`/`Conflict` bookkeeping would silently drift from
/// the buffer it now describes. When merge is NOT active, the jump may have
/// unwound PAST a previously-adopted resolution (`rune-db`'s undo-unwind
/// override upgrades `DiskAhead` back to `Diverged` in that case) — a fresh
/// probe re-lights the footer's disk-changed hint so the user is offered
/// `^M` again rather than the hint staying stale.
fn resync_after_journal_jump(
    app: &mut App,
    id: DocumentId,
    affected: Option<std::ops::Range<usize>>,
) {
    if matches!(&app.merge, crate::merge::MergeState::Active { doc, .. } if *doc == id) {
        crate::merge::resync(app, id, affected);
    } else {
        crate::db_enqueue::probe(app, id);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use rune_vfs::Mem;
    use std::sync::Arc;

    fn app_with(content: &str, cursor_offset: usize) -> App {
        let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
        let id = app.active;
        app.doc_mut(id).unwrap().cursors = CursorSet::new(cursor_offset.min(content.len()));
        app.doc_mut(id).unwrap().viewport.set_size(80, 23);
        app
    }

    /// End-to-end regression for the illegal-edit-set hazard
    /// `edit_core::coalesce_touching_edits` closes at the batch-
    /// construction chokepoint (see that module's own unit tests for the
    /// narrower proof): two cursors at ADJACENT byte offsets — a
    /// perfectly ordinary, non-merged `CursorSet` state — both pressing
    /// Delete-forward in the same keystroke. Each cursor's own derived
    /// delete range is one byte (`[0,1)` and `[1,2)`); those two ranges
    /// touch. Without the fix, the journaled step holds two `AppliedEdit`s
    /// that share the same post-edit `start`, and `redo` (which calls
    /// `undo::reapply`) panics on its precondition `debug_assert!` in this
    /// (debug) test build — `crates/rune-fuzz` artifact `no-panic-
    /// 7f29861c`, checked in as `repros/no-panic-01.rune`, reached this
    /// exact shape via multicursor-add-below + typing + Cmd+X (cut, which
    /// deletes each cursor's whole current line when it has no selection
    /// — the same "adjacent derived ranges" hazard applied to whole
    /// lines instead of single runes).
    #[test]
    fn delete_right_with_adjacent_cursors_journals_one_edit_and_redoes_cleanly() {
        let mut app = app_with("abcd", 0);
        let id = app.active;
        let two = CursorSet::new(0).add(Cursor {
            position: 1,
            anchor: 1,
            desired_col: 0,
            id: 0,
        });
        assert_eq!(
            two.len(),
            2,
            "fixture must really hold two adjacent cursors"
        );
        app.doc_mut(id).unwrap().cursors = two;

        delete_right(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "cd");
        assert_eq!(app.doc(id).unwrap().journal.len(), 1);
        let step_edit_starts: Vec<usize> = app
            .doc(id)
            .unwrap()
            .journal
            .undo_peek()
            .map(|(step, _)| step.edits.iter().map(|e| e.start).collect())
            .unwrap_or_default();
        assert_eq!(
            step_edit_starts.len(),
            1,
            "the two touching per-cursor deletes must be journaled as ONE edit, \
             not two sharing a post-edit start: {step_edit_starts:?}"
        );

        undo(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "abcd");

        // Panics here (debug_assert in `undo::reapply`) if the two
        // cursors' adjacent delete-right ranges were journaled as two
        // AppliedEdits sharing a post-edit `start` instead of being
        // coalesced into one.
        redo(&mut app, id);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "cd");
    }
}
