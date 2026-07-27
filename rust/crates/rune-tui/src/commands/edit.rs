//! Editing (insert/backspace/delete/newline/indent/outdent) and undo/redo
//! (WP7). Port of `pkg/ui/components/textedit/commands_edit.go`,
//! `commands_edit_lines_indent.go`, `commands_edit_lines.go`'s
//! `perCursorSelectionEdits`/`perLineEdits`/`buildEditResultFromInfos`
//! drivers, and the peek-then-commit undo/redo discipline of
//! `pkg/ui/pages/workspace/workspace_undo.go:31-142`.
//!
//! Workspace-coupled (plan WP1 decision 4): every function here takes
//! `(app: &mut App, id: DocumentId)` — every mutation funnels through
//! `commit_edit_batch`, which also touches `app.db`/`app.status_message`/
//! the dirty cache, so unlike `commands::nav` this module can't work off a
//! bare `&mut Document`. Internally, functions borrow `app.doc_mut(id)`
//! SEQUENTIALLY — mutate the doc, let that borrow end, then call
//! `db::append_edit(app, id, ...)`/`save::recompute_dirty(app, id)` — never
//! a split-borrow context type.
//!
//! Backspace/delete-right are RUNE-aware, not grapheme-cluster-aware —
//! this matches Go exactly: `commands_nav.go:prevRuneOffset`/
//! `nextRuneOffset` (which `execDeleteLeft`/`execDeleteRight` call) decode
//! one UTF-8 rune at a time via `utf8.DecodeLastRuneInString`, with no
//! grapheme-CLUSTER SEGMENTATION anywhere in the Go source's delete path.
//! The Go tree does use `Grapheme` as a struct field name elsewhere
//! (`textedit/cell.go`'s per-`Cell` rendered glyph string,
//! `markdownedit/render_image.go`, `internal/fuzz/artifact/artifact.go`'s
//! serialized snapshot of it) — but those are RENDER-TIME display-cell
//! payloads (what glyph a cell shows), never consulted by
//! `execDeleteLeft`/`execDeleteRight`'s offset computation. A ZWJ emoji
//! family sequence therefore deletes one codepoint per Backspace in both
//! implementations, not the whole cluster — ported 1:1, not "improved",
//! since drifting from Go here would be a silent behavior change the plan
//! didn't ask for.

use std::collections::HashSet;

use rune_core::buffer::{AppliedEdit, Buffer, Edit};
use rune_core::cursor::{Cursor, CursorSet};
use rune_core::undo::Step;

use crate::app::{App, StatusSource};
use crate::commands::nav;
use crate::db;
use crate::document::DocumentId;
use crate::save;

/// Port of `commands_edit_lines.go:sortInfosDescending` +
/// `buildEditResultFromInfos` + `textedit.go:applyOperation`'s edit-apply
/// branch + `commitEdits`: batch-apply the collected `(Edit, owning cursor
/// id)` pairs, recompute each surviving cursor's post-edit position, and
/// journal the step. `AppliedEdit::end` (`start + insert.len()`, already in
/// POST-edit coordinates per `buffer.rs`'s own docs) IS the post-edit
/// caret position for that edit — using it directly is simpler than
/// re-deriving Go's `computePostEditCursors` shift accumulation and can
/// never disagree with what `Buffer::apply_edits` actually did, since it
/// comes from the same call.
///
/// Deliberate improvement over Go's `applyOperation`: Go assigns
/// `result.Operation.Cursors` (computed as if the edit succeeded)
/// UNCONDITIONALLY, even when `ApplyEdits` itself returned an error — a
/// dead branch in practice (a cursor-derived edit batch is always
/// in-bounds), but not a Rust type-state to leave standing. Here cursors
/// only ever change on `Ok`; a rejected batch surfaces to the status line
/// and leaves buffer/cursors untouched (CONSTITUTION §1.3: "fail fast on
/// data risk", the same discipline `undo`/`redo` below already follow).
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
/// typing, backspace/delete, indent/outdent, cut, paste — funnels through
/// `per_cursor_selection_edits`/`per_line_edits` into this one function, so
/// checking `app.doc(id).read_only` HERE (before anything else — no
/// partial work, no journal entry, no cursor change) makes "a read-only
/// document got mutated" unreachable regardless of which command tried it,
/// rather than relying on every call site to remember its own guard (see
/// `Document::read_only`'s docs for the bug this closes and why `undo`/
/// `redo` below are deliberately exempt).
fn commit_edit_batch(
    app: &mut App,
    id: DocumentId,
    mut infos: Vec<(Edit, u32)>,
    cursors_before: CursorSet,
) {
    if app.doc(id).read_only || infos.is_empty() {
        return;
    }
    infos.sort_by(|a, b| b.0.start.cmp(&a.0.start).then(b.0.end.cmp(&a.0.end)));

    let edits: Vec<Edit> = infos.iter().map(|(e, _)| e.clone()).collect();
    let ids: Vec<u32> = infos.iter().map(|(_, cid)| *cid).collect();

    match app.doc(id).buffer.apply_edits(&edits) {
        Ok((new_buf, applied)) => {
            let new_cursors: Vec<Cursor> = applied
                .iter()
                .zip(ids.iter())
                .map(|(ae, &cid)| Cursor {
                    position: ae.end,
                    anchor: ae.end,
                    desired_col: 0,
                    id: cid,
                })
                .collect();
            app.doc_mut(id).buffer = new_buf;
            app.doc_mut(id).cursors = CursorSet::new_from(&new_cursors);
            let cursors_after = app.doc(id).cursors.all();
            app.doc_mut(id).journal.push(Step {
                edits: applied.clone(),
                cursors_before: cursors_before.all(),
                cursors_after: cursors_after.clone(),
            });
            // Async replica journaling (plan WP5.S3): the LOCAL journal
            // above is already the authoritative, synchronous source of
            // truth — this enqueue can never roll it back, only mark the
            // store degraded on failure (`db::append_edit`'s doc comment).
            let local_pos = app.doc(id).journal.pos();
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

/// Port of `commands_edit_lines.go:perCursorSelectionEdits`: one edit per
/// cursor, replacing its selection when it has one, or `bare`'s caller-
/// chosen range otherwise. `bare` returning `None` skips that cursor
/// entirely (e.g. Backspace at buffer start).
fn per_cursor_selection_edits(
    app: &mut App,
    id: DocumentId,
    text_for: impl Fn(usize, &Cursor, &Buffer) -> String,
    bare: impl Fn(&Buffer, &Cursor) -> Option<(usize, usize)>,
) {
    let cursors_before = app.doc(id).cursors.clone();
    let all = cursors_before.all();
    if all.is_empty() {
        return;
    }

    let mut infos: Vec<(Edit, u32)> = Vec::new();
    for (i, c) in all.iter().enumerate() {
        let buf = &app.doc(id).buffer;
        let edit = if c.has_selection() {
            let start = c.selection_start();
            let end = nav::selection_end_inclusive(c, buf);
            Edit {
                start,
                end,
                insert: text_for(i, c, buf),
                cursor_id: c.id,
            }
        } else if let Some((start, end)) = bare(buf, c) {
            Edit {
                start,
                end,
                insert: text_for(i, c, buf),
                cursor_id: c.id,
            }
        } else {
            continue;
        };
        infos.push((edit, c.id));
    }

    commit_edit_batch(app, id, infos, cursors_before);
}

/// Port of `commands_edit_lines.go:perLineEdits` with `dedupe=true` (every
/// caller in this file dedupes — Go's only `dedupe=false` caller,
/// clone-line-up/down, is out of Phase-1 scope).
fn per_line_edits(app: &mut App, id: DocumentId, build: impl Fn(usize, &Buffer) -> Option<Edit>) {
    let cursors_before = app.doc(id).cursors.clone();
    let all = cursors_before.all();
    if all.is_empty() {
        return;
    }

    let mut infos: Vec<(Edit, u32)> = Vec::new();
    let mut seen: HashSet<usize> = HashSet::new();
    for c in &all {
        let bp = app.doc(id).buffer.offset_to_line_col(c.position);
        if !seen.insert(bp.line) {
            continue;
        }
        if let Some(edit) = build(bp.line, &app.doc(id).buffer) {
            infos.push((edit, c.id));
        }
    }

    commit_edit_batch(app, id, infos, cursors_before);
}

/// Port of `commands_edit.go:execInsertChar`, generalized to arbitrary text
/// so it doubles as the selection-replacing insert path for bracketed
/// paste (`Msg::Paste`, plan Context: "Bracketed-paste `Msg::Paste` may
/// insert text through the same insert path").
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

/// Port of `commands_edit.go:execInsertChar`.
pub fn insert_char(app: &mut App, id: DocumentId, ch: char) {
    let mut buf = [0u8; 4];
    insert_text(app, id, ch.encode_utf8(&mut buf));
}

/// Port of `commands_edit.go:execNewline` — the Enter hardcoded fast path
/// (plan Context, "Hardcoded fast paths outside the resolver"): inserts a
/// newline plus the CURRENT line's own leading whitespace, preserving
/// indentation.
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

/// Port of `commands_clipboard.go:buildDeleteEdits`, reused by
/// `commands::clipboard::cut` (WP8): deletes each cursor's selection, or —
/// with no selection — its whole current line including the trailing `\n`
/// (`nav::line_range_incl_newline`, the same range `copy_entire_line` used
/// to build the text cut just copied — so cut always removes precisely
/// what it captured).
pub(crate) fn delete_selection_or_line(app: &mut App, id: DocumentId) {
    per_cursor_selection_edits(
        app,
        id,
        |_i, _c, _buf| String::new(),
        |buf, c| Some(nav::line_range_incl_newline(buf, c.position)),
    );
}

/// Port of `commands_edit.go:execDeleteLeft` (Backspace).
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

/// Port of `commands_edit.go:execDeleteRight` (Delete).
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

/// Port of `commands_edit_lines_indent.go:execIndentLine` (Tab).
pub fn indent(app: &mut App, id: DocumentId) {
    per_line_edits(app, id, |line, buf| {
        let line_start = buf.line_start(line);
        Some(Edit {
            start: line_start,
            end: line_start,
            insert: "\t".to_string(),
            cursor_id: 0,
        })
    });
}

/// Port of `commands_edit_lines_indent.go:execDedentLine` (Shift+Tab):
/// removes up to one leading tab, or up to 4 leading spaces if the line
/// starts with at least 4 of them.
pub fn outdent(app: &mut App, id: DocumentId) {
    per_line_edits(app, id, dedent_edit_for_line);
}

fn dedent_edit_for_line(line: usize, buf: &Buffer) -> Option<Edit> {
    let line_start = buf.line_start(line);
    let line_end = buf.line_end(line);
    let line_text = buf.slice(line_start, line_end).unwrap_or("");

    let mut indent_end = 0usize;
    for ch in line_text.chars() {
        if ch == '\t' || ch == ' ' {
            indent_end += ch.len_utf8();
        } else {
            break;
        }
    }
    if indent_end == 0 {
        return None;
    }

    let mut remove = 1usize;
    if indent_end >= 4 {
        let space_run = line_text.bytes().take_while(|&b| b == b' ').count();
        if space_run >= 4 {
            remove = 4;
        }
    }
    if indent_end < remove {
        remove = indent_end;
    }

    Some(Edit {
        start: line_start,
        end: line_start + remove,
        insert: String::new(),
        cursor_id: 0,
    })
}

/// Port of `workspace_undo.go:handleUndo`: peek the target step (without
/// moving the journal), apply its inverse to the buffer, and commit the
/// position move ONLY if the buffer edit succeeds (§1.4.8) — a failed
/// apply surfaces a status-line error and leaves the journal position (and
/// buffer) untouched, so the journal never runs ahead of the buffer. Same
/// status-message ownership rule as `commit_edit_batch` (F2): success never
/// clears `app.status_message` — only this function's own failure path
/// writes it.
pub fn undo(app: &mut App, id: DocumentId) {
    let Some((step, new_pos)) = app.doc(id).journal.undo_peek() else {
        return;
    };
    let edits: Vec<AppliedEdit> = step.edits.clone();
    let cursors_before: Vec<Cursor> = step.cursors_before.clone();

    match rune_core::undo::apply_inverse(&app.doc(id).buffer, &edits) {
        Ok(new_buf) => {
            app.doc_mut(id).buffer = new_buf;
            app.doc_mut(id).cursors = CursorSet::new_from(&cursors_before);
            app.doc_mut(id).journal.move_pos(new_pos);
            db::move_undo_pos(app, id, new_pos);
            save::recompute_dirty(app, id);
        }
        Err(e) => {
            app.set_status(format!("undo failed: {e}"), StatusSource::Other);
        }
    }
}

/// Port of `workspace_undo.go:handleRedo` — mirrors `undo` above: reapply
/// the step forward, commit the position move only on success. Same
/// status-message ownership rule as `commit_edit_batch`/`undo` (F2).
pub fn redo(app: &mut App, id: DocumentId) {
    let Some((step, new_pos)) = app.doc(id).journal.redo_peek() else {
        return;
    };
    let edits: Vec<AppliedEdit> = step.edits.clone();
    let cursors_after: Vec<Cursor> = step.cursors_after.clone();

    match rune_core::undo::reapply(&app.doc(id).buffer, &edits) {
        Ok(new_buf) => {
            app.doc_mut(id).buffer = new_buf;
            app.doc_mut(id).cursors = CursorSet::new_from(&cursors_after);
            app.doc_mut(id).journal.move_pos(new_pos);
            db::move_undo_pos(app, id, new_pos);
            save::recompute_dirty(app, id);
        }
        Err(e) => {
            app.set_status(format!("redo failed: {e}"), StatusSource::Other);
        }
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
        app.doc_mut(id).cursors = CursorSet::new(cursor_offset.min(content.len()));
        app.doc_mut(id).viewport.set_size(80, 23);
        app
    }

    #[test]
    fn insert_char_moves_caret_past_the_inserted_char() {
        let mut app = app_with("ac", 1);
        let id = app.active;
        insert_char(&mut app, id, 'b');
        assert_eq!(app.doc(id).buffer.content(), "abc");
        assert_eq!(app.doc(id).cursors.primary().position, 2);
        assert_eq!(app.doc(id).journal.len(), 1);
    }

    #[test]
    fn insert_char_replaces_a_selection() {
        let mut app = app_with("hello world", 0);
        let id = app.active;
        app.doc_mut(id).cursors = CursorSet::new(0).map(|c| Cursor {
            anchor: 0,
            position: 5,
            ..c
        });
        insert_char(&mut app, id, 'X');
        assert_eq!(app.doc(id).buffer.content(), "X world");
        assert_eq!(app.doc(id).cursors.primary().position, 1);
    }

    #[test]
    fn backspace_deletes_one_rune_never_splitting_a_multibyte_char() {
        let mut app = app_with("a\u{6c49}", "a".len() + '\u{6c49}'.len_utf8());
        let id = app.active;
        delete_left(&mut app, id);
        assert_eq!(app.doc(id).buffer.content(), "a");
    }

    #[test]
    fn backspace_at_buffer_start_is_a_no_op() {
        let mut app = app_with("abc", 0);
        let id = app.active;
        delete_left(&mut app, id);
        assert_eq!(app.doc(id).buffer.content(), "abc");
        assert_eq!(app.doc(id).journal.len(), 0);
    }

    #[test]
    fn delete_right_removes_one_rune_forward() {
        let mut app = app_with("abc", 0);
        let id = app.active;
        delete_right(&mut app, id);
        assert_eq!(app.doc(id).buffer.content(), "bc");
        assert_eq!(app.doc(id).cursors.primary().position, 0);
    }

    #[test]
    fn newline_preserves_current_line_indentation() {
        let mut app = app_with("  indented", 10);
        let id = app.active;
        newline(&mut app, id);
        assert_eq!(app.doc(id).buffer.content(), "  indented\n  ");
        assert_eq!(app.doc(id).cursors.primary().position, 13);
    }

    #[test]
    fn indent_inserts_a_leading_tab() {
        let mut app = app_with("hello", 2);
        let id = app.active;
        indent(&mut app, id);
        assert_eq!(app.doc(id).buffer.content(), "\thello");
    }

    #[test]
    fn outdent_removes_one_leading_tab() {
        let mut app = app_with("\thello", 3);
        let id = app.active;
        outdent(&mut app, id);
        assert_eq!(app.doc(id).buffer.content(), "hello");
    }

    #[test]
    fn outdent_removes_up_to_four_leading_spaces() {
        let mut app = app_with("    hello", 5);
        let id = app.active;
        outdent(&mut app, id);
        assert_eq!(app.doc(id).buffer.content(), "hello");
    }

    #[test]
    fn outdent_on_a_line_with_no_indentation_is_a_no_op() {
        let mut app = app_with("hello", 0);
        let id = app.active;
        outdent(&mut app, id);
        assert_eq!(app.doc(id).buffer.content(), "hello");
        assert_eq!(app.doc(id).journal.len(), 0);
    }

    #[test]
    fn undo_then_redo_round_trips_content_and_cursors() {
        let mut app = app_with("hello", 5);
        let id = app.active;
        insert_char(&mut app, id, '!');
        assert_eq!(app.doc(id).buffer.content(), "hello!");

        undo(&mut app, id);
        assert_eq!(app.doc(id).buffer.content(), "hello");
        assert_eq!(app.doc(id).cursors.primary().position, 5);

        redo(&mut app, id);
        assert_eq!(app.doc(id).buffer.content(), "hello!");
        assert_eq!(app.doc(id).cursors.primary().position, 6);
    }

    #[test]
    fn undo_with_empty_journal_is_a_no_op() {
        let mut app = app_with("hello", 0);
        let id = app.active;
        undo(&mut app, id);
        assert_eq!(app.doc(id).buffer.content(), "hello");
    }

    #[test]
    fn cjk_and_emoji_round_trip_byte_exact_through_undo() {
        let mut app = app_with("汉字 👩‍👩‍👧‍👦", 0);
        let id = app.active;
        let original = app.doc(id).buffer.content().to_string();
        insert_char(&mut app, id, 'x');
        undo(&mut app, id);
        assert_eq!(app.doc(id).buffer.content(), original);
    }

    /// Regression for F2: a successful edit must not clobber an unrelated
    /// (e.g. save-failure) status message — only this module's OWN failure
    /// paths may write `status_message`.
    #[test]
    fn a_successful_edit_does_not_clear_an_unrelated_status_message() {
        let mut app = app_with("hello", 5);
        let id = app.active;
        app.status_message = Some("save failed: disk full".to_string());

        insert_char(&mut app, id, '!');
        assert_eq!(app.doc(id).buffer.content(), "hello!");
        assert_eq!(
            app.status_message.as_deref(),
            Some("save failed: disk full"),
            "an unrelated save-failure message must survive a successful edit"
        );

        undo(&mut app, id);
        assert_eq!(
            app.status_message.as_deref(),
            Some("save failed: disk full"),
            "an unrelated save-failure message must survive a successful undo"
        );

        redo(&mut app, id);
        assert_eq!(
            app.status_message.as_deref(),
            Some("save failed: disk full"),
            "an unrelated save-failure message must survive a successful redo"
        );
    }

    /// Regression for F1: a read-only `Document` rejects every mutating
    /// command at the `commit_edit_batch` chokepoint — buffer content,
    /// buffer version, and the undo journal are all left untouched by
    /// typing, Backspace, and Indent alike (an earlier version guarded only
    /// `commands::clipboard::handle_paste_content`, leaving these three
    /// paths able to mutate a "read-only" document).
    #[test]
    fn read_only_blocks_typing_backspace_and_indent() {
        let mut app = app_with("hello", 5);
        let id = app.active;
        app.doc_mut(id).read_only = true;
        let before_content = app.doc(id).buffer.content().to_string();
        let before_version = app.doc(id).buffer.version();

        insert_char(&mut app, id, '!');
        assert_eq!(app.doc(id).buffer.content(), before_content, "typing");

        delete_left(&mut app, id);
        assert_eq!(app.doc(id).buffer.content(), before_content, "backspace");

        indent(&mut app, id);
        assert_eq!(app.doc(id).buffer.content(), before_content, "indent");

        outdent(&mut app, id);
        assert_eq!(app.doc(id).buffer.content(), before_content, "outdent");

        delete_right(&mut app, id);
        assert_eq!(app.doc(id).buffer.content(), before_content, "delete-right");

        newline(&mut app, id);
        assert_eq!(app.doc(id).buffer.content(), before_content, "newline");

        assert_eq!(
            app.doc(id).buffer.version(),
            before_version,
            "a rejected mutation must never bump the buffer version"
        );
        assert_eq!(
            app.doc(id).journal.len(),
            0,
            "a rejected mutation must never be journaled"
        );
    }

    /// Regression for F1 (Go parity): `undo`/`redo` are deliberately NOT
    /// gated by `read_only` — Go's own `ApplyInverse`/`Reapply`
    /// (`edit_primitives.go:51,86`) bypass `m.readOnly` the same way
    /// `ReplaceRange` (`edit_primitives.go:25`) does not. A document that
    /// became read-only after edits were already journaled (e.g. Go's Help
    /// view is generated fresh and never has journal history, but this
    /// property must hold regardless) must still let undo/redo walk that
    /// history.
    #[test]
    fn undo_and_redo_are_not_blocked_by_read_only() {
        let mut app = app_with("hello", 5);
        let id = app.active;
        insert_char(&mut app, id, '!');
        assert_eq!(app.doc(id).buffer.content(), "hello!");

        app.doc_mut(id).read_only = true;

        undo(&mut app, id);
        assert_eq!(
            app.doc(id).buffer.content(),
            "hello",
            "undo must not be blocked by read_only"
        );

        redo(&mut app, id);
        assert_eq!(
            app.doc(id).buffer.content(),
            "hello!",
            "redo must not be blocked by read_only"
        );
    }
}
