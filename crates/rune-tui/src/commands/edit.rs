//! Editing (insert/backspace/delete/delete-word/newline) and undo/redo:
//! per-cursor edit commands run through a shared driver
//! (`per_cursor_selection_edits`), and undo/redo follow a peek-then-commit
//! discipline (see `undo`/`redo` below). The shared buffer-mutation
//! chokepoint (`apply_edit_batch_with_cursors`/`commit_edit_batch`) lives
//! in the sibling `edit_core` module, and the line-oriented commands
//! (indent/outdent, delete-line, clone/move-line) live in the sibling
//! `edit_lines` module: one `edit_lines` boundary was not enough to bring
//! this file itself under the 500-line budget, hence the second split
//! into `edit_core` — see each module's own doc for why the boundary sits
//! there.
//!
//! Workspace-coupled: every function here takes
//! `edit_core::commit_edit_batch`, which also touches `app.db`/the
//! message log/the dirty cache, so unlike `commands::nav` this
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
use rune_core::cursor::{Cursor, CursorId, CursorSet};
use rune_core::undo::{EditKind, Journal};

use crate::app::App;
use crate::commands::edit_core::commit_edit_batch;
use crate::commands::nav;
use crate::commands::nav_line;
use crate::db_enqueue as db;
use crate::document::{DocumentId, ReadOnly};
use crate::materialize_ack;
use crate::messages;
use crate::undogroup::{self, Direction, Tier};

/// One edit per cursor, replacing its selection when it has one, or
/// `bare`'s caller-chosen range otherwise. `bare` returning `None` skips
/// that cursor entirely (e.g. Backspace at buffer start).
pub(crate) fn per_cursor_selection_edits(
    app: &mut App,
    id: DocumentId,
    kind: EditKind,
    text_for: impl Fn(usize, &Cursor, &Buffer) -> String,
    bare: impl Fn(&Buffer, &Cursor) -> Option<(usize, usize)>,
) {
    let Some(doc) = app.doc(id) else { return };
    let cursors_before = doc.cursors.clone();
    let all = cursors_before.all();
    if all.is_empty() {
        return;
    }

    let mut infos: Vec<(Edit, CursorId)> = Vec::new();
    for (i, c) in all.iter().enumerate() {
        let Some(doc) = app.doc(id) else { return };
        let buf = &doc.buffer;
        let edit = if c.has_selection() {
            let (start, end) = c.selection_range();
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

    let _ = commit_edit_batch(app, id, infos, &cursors_before, kind);
}

/// Generalized to arbitrary text so it doubles as the selection-replacing
/// insert path for bracketed paste (`Msg::Paste`, plan Context:
/// "Bracketed-paste `Msg::Paste` may insert text through the same insert
/// path") — `kind` tells the two apart for the undo journal.
pub fn insert_text(app: &mut App, id: DocumentId, text: &str, kind: EditKind) {
    if text.is_empty() {
        return;
    }
    per_cursor_selection_edits(
        app,
        id,
        kind,
        move |_i, _c, _buf| text.to_string(),
        |_buf, c| Some((c.position, c.position)),
    );
}

pub fn insert_char(app: &mut App, id: DocumentId, ch: char) {
    let mut buf = [0u8; 4];
    insert_text(app, id, ch.encode_utf8(&mut buf), EditKind::Insert);
}

/// The Enter hardcoded fast path (plan Context, "Hardcoded fast paths
/// outside the resolver"): inserts a newline plus the CURRENT line's own
/// leading whitespace, preserving indentation.
pub fn newline(app: &mut App, id: DocumentId) {
    per_cursor_selection_edits(
        app,
        id,
        EditKind::Insert,
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

/// Reused by `commands::clipboard::cut`: deletes each cursor's
/// selection, or — with no selection — its whole current line including
/// the trailing `\n` (`nav_line::line_range_incl_newline`, the same range
/// `copy_entire_line` used to build the text cut just copied — so cut
/// always removes precisely what it captured).
pub(crate) fn delete_selection_or_line(app: &mut App, id: DocumentId) {
    per_cursor_selection_edits(
        app,
        id,
        EditKind::Cut,
        |_i, _c, _buf| String::new(),
        |buf, c| Some(nav_line::line_range_incl_newline(buf, c.position)),
    );
}

pub fn delete_left(app: &mut App, id: DocumentId) {
    per_cursor_selection_edits(
        app,
        id,
        EditKind::DeleteLeft,
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
        EditKind::DeleteRight,
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

/// Deletes the selection when the cursor has one (same
/// selection-first rule every `per_cursor_selection_edits` caller
/// shares); otherwise deletes from `nav::word_left_offset` up to the
/// caret — one word, not one rune.
pub fn delete_word_left(app: &mut App, id: DocumentId) {
    per_cursor_selection_edits(
        app,
        id,
        EditKind::DeleteLeft,
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
        EditKind::DeleteRight,
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

fn merge_active_on(app: &App, id: DocumentId) -> bool {
    matches!(&app.merge, crate::merge::MergeState::Active { doc, .. } if *doc == id)
}

fn ladder_press(
    app: &mut App,
    id: DocumentId,
    direction: Direction,
    steps_for: fn(&Journal, Tier) -> usize,
) -> usize {
    let now = app.clock.now();
    let Some(doc) = app.doc_mut(id) else {
        return 0;
    };
    let reset = doc.ladder_direction != Some(direction)
        || doc
            .ladder_pressed_at
            .is_none_or(|last| now.duration_since(last) >= undogroup::LADDER_RESET);
    if reset {
        doc.ladder_presses = 0;
        if direction == Direction::Undo {
            doc.ladder_anchor = Some(doc.journal.pos());
        }
    }
    doc.ladder_direction = Some(direction);
    let tier = undogroup::tier_for(doc.ladder_presses);
    let mut count = steps_for(&doc.journal, tier);
    if direction == Direction::Redo {
        match doc.ladder_anchor {
            Some(anchor) if doc.journal.pos() < anchor => {
                count = count.min(anchor - doc.journal.pos());
            }
            _ => doc.ladder_anchor = None,
        }
    }
    doc.ladder_presses += 1;
    doc.ladder_pressed_at = Some(now);
    count
}

pub fn undo(app: &mut App, id: DocumentId) {
    if app.refuse_if_preview(id) {
        return;
    }
    let Some(read_only) = app.doc(id).map(|doc| doc.read_only) else {
        return;
    };
    if read_only == ReadOnly::Reading {
        app.refuse_if_read_only(read_only);
        return;
    }

    if app
        .doc(id)
        .is_some_and(|doc| doc.journal.undo_peek().is_none())
    {
        messages::info(app, "nothing to undo");
        return;
    }

    let count = if merge_active_on(app, id) {
        1
    } else {
        ladder_press(app, id, Direction::Undo, undogroup::steps_for)
    };

    let pre_content = app
        .doc(id)
        .map(|doc| doc.buffer.content().to_string())
        .unwrap_or_default();
    let mut reached = None;
    for _ in 0..count {
        let Some(doc) = app.doc(id) else { break };
        let Some((step, token)) = doc.journal.undo_peek() else {
            break;
        };
        let edits: Vec<AppliedEdit> = step.edits.clone();
        let cursors_before: Vec<Cursor> = step.cursors_before.clone();
        let deltas = crate::merge::ranges::inverse_deltas(&edits);

        match rune_core::undo::apply_inverse(&doc.buffer, &edits) {
            Ok(new_buf) => {
                let target = token.target();
                let Some(doc) = app.doc_mut(id) else { break };
                doc.buffer = new_buf;
                doc.cursors = CursorSet::new_from(&cursors_before);
                doc.journal.commit(token);
                reached = Some(target);
                for ae in &edits {
                    app.nav_history
                        .shift(id, ae.start, ae.insert.len(), ae.deleted.len());
                }
                resync_after_journal_jump(app, id, &deltas);
            }
            Err(e) => {
                messages::error(app, format!("undo failed: {e}"));
                break;
            }
        }
    }

    if let Some(target) = reached {
        db::move_undo_pos(app, id, target, &pre_content);
        materialize_ack::recompute_dirty(app, id);
    }
}

pub fn redo(app: &mut App, id: DocumentId) {
    if app.refuse_if_preview(id) {
        return;
    }
    let Some(read_only) = app.doc(id).map(|doc| doc.read_only) else {
        return;
    };
    if read_only == ReadOnly::Reading {
        app.refuse_if_read_only(read_only);
        return;
    }

    if app
        .doc(id)
        .is_some_and(|doc| doc.journal.redo_peek().is_none())
    {
        messages::info(app, "nothing to redo");
        return;
    }

    let count = if merge_active_on(app, id) {
        1
    } else {
        ladder_press(app, id, Direction::Redo, undogroup::steps_for_redo)
    };

    let pre_content = app
        .doc(id)
        .map(|doc| doc.buffer.content().to_string())
        .unwrap_or_default();
    let mut reached = None;
    for _ in 0..count {
        let Some(doc) = app.doc(id) else { break };
        let Some((step, token)) = doc.journal.redo_peek() else {
            break;
        };
        let edits: Vec<AppliedEdit> = step.edits.clone();
        let cursors_after: Vec<Cursor> = step.cursors_after.clone();
        let deltas = crate::merge::ranges::forward_deltas(&edits);

        match rune_core::undo::reapply(&doc.buffer, &edits) {
            Ok(new_buf) => {
                let target = token.target();
                let Some(doc) = app.doc_mut(id) else { break };
                doc.buffer = new_buf;
                doc.cursors = CursorSet::new_from(&cursors_after);
                doc.journal.commit(token);
                reached = Some(target);
                for ae in edits.iter().rev() {
                    app.nav_history
                        .shift(id, ae.start, ae.deleted.len(), ae.insert.len());
                }
                resync_after_journal_jump(app, id, &deltas);
            }
            Err(e) => {
                messages::error(app, format!("redo failed: {e}"));
                break;
            }
        }
    }

    if let Some(target) = reached {
        db::move_undo_pos(app, id, target, &pre_content);
        materialize_ack::recompute_dirty(app, id);
    }
}

fn resync_after_journal_jump(
    app: &mut App,
    id: DocumentId,
    deltas: &[crate::merge::ranges::Delta],
) {
    if merge_active_on(app, id) {
        crate::merge::ranges::rederive_after_jump(app, id, deltas);
    } else {
        crate::db_enqueue::probe(app, id);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::commands::test_support::selecting;
    use rune_core::cursor::CursorSpec;
    use rune_vfs::Mem;
    use std::sync::Arc;

    fn app_with(content: &str, cursor_offset: usize) -> App {
        let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
        let id = app.active;
        app.doc_mut(id).unwrap().cursors = CursorSet::new(cursor_offset.min(content.len()));
        app.doc_mut(id).unwrap().viewport.set_size(80, 23);
        app
    }

    #[test]
    fn backspace_on_a_reversed_selection_removes_exactly_the_highlighted_range() {
        let mut app = app_with("hello world", 0);
        let id = app.active;
        selecting(&mut app, id, 5, 0);

        delete_left(&mut app, id);

        assert_eq!(app.doc(id).unwrap().buffer.content(), " world");
    }

    #[test]
    fn typing_over_a_reversed_selection_replaces_exactly_the_highlighted_range() {
        let mut app = app_with("hello world", 0);
        let id = app.active;
        selecting(&mut app, id, 5, 0);

        insert_char(&mut app, id, 'x');

        assert_eq!(app.doc(id).unwrap().buffer.content(), "x world");
    }

    #[test]
    fn typing_over_a_forward_selection_replaces_exactly_the_highlighted_range() {
        let mut app = app_with("hello world", 0);
        let id = app.active;
        selecting(&mut app, id, 0, 5);

        insert_char(&mut app, id, 'x');

        assert_eq!(app.doc(id).unwrap().buffer.content(), "x world");
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
        let two = CursorSet::new(0).add(CursorSpec {
            position: 1,
            anchor: 1,
            desired_col: 0,
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
