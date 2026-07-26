//! Editing (insert/backspace/delete/newline/indent/outdent) and undo/redo
//! (WP7). Port of `pkg/ui/components/textedit/commands_edit.go`,
//! `commands_edit_lines_indent.go`, `commands_edit_lines.go`'s
//! `perCursorSelectionEdits`/`perLineEdits`/`buildEditResultFromInfos`
//! drivers, and the peek-then-commit undo/redo discipline of
//! `pkg/ui/pages/workspace/workspace_undo.go:31-142`.
//!
//! Backspace/delete-right are RUNE-aware, not grapheme-cluster-aware —
//! this matches Go exactly: `commands_nav.go:prevRuneOffset`/
//! `nextRuneOffset` (which `execDeleteLeft`/`execDeleteRight` call) decode
//! one UTF-8 rune at a time via `utf8.DecodeLastRuneInString`, with no
//! grapheme-segmentation anywhere in the Go source (confirmed: no
//! reference to "grapheme" in the Go tree). A ZWJ emoji family sequence
//! therefore deletes one codepoint per Backspace in both implementations,
//! not the whole cluster — ported 1:1, not "improved", since drifting from
//! Go here would be a silent behavior change the plan didn't ask for.

use std::collections::HashSet;

use rune_core::buffer::{AppliedEdit, Buffer, Edit};
use rune_core::cursor::{Cursor, CursorSet};
use rune_core::undo::Step;

use crate::app::App;
use crate::commands::nav;

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
fn commit_edit_batch(app: &mut App, mut infos: Vec<(Edit, u32)>, cursors_before: CursorSet) {
    if infos.is_empty() {
        return;
    }
    infos.sort_by(|a, b| b.0.start.cmp(&a.0.start).then(b.0.end.cmp(&a.0.end)));

    let edits: Vec<Edit> = infos.iter().map(|(e, _)| e.clone()).collect();
    let ids: Vec<u32> = infos.iter().map(|(_, id)| *id).collect();

    match app.editor.buffer.apply_edits(&edits) {
        Ok((new_buf, applied)) => {
            let new_cursors: Vec<Cursor> = applied
                .iter()
                .zip(ids.iter())
                .map(|(ae, &id)| Cursor {
                    position: ae.end,
                    anchor: ae.end,
                    desired_col: 0,
                    id,
                })
                .collect();
            app.editor.buffer = new_buf;
            app.editor.cursors = CursorSet::new_from(&new_cursors);
            app.editor.journal.push(Step {
                edits: applied,
                cursors_before: cursors_before.all(),
                cursors_after: app.editor.cursors.all(),
            });
            app.status_message = None;
        }
        Err(e) => {
            app.status_message = Some(format!("edit failed: {e}"));
        }
    }
}

/// Port of `commands_edit_lines.go:perCursorSelectionEdits`: one edit per
/// cursor, replacing its selection when it has one, or `bare`'s caller-
/// chosen range otherwise. `bare` returning `None` skips that cursor
/// entirely (e.g. Backspace at buffer start).
fn per_cursor_selection_edits(
    app: &mut App,
    text_for: impl Fn(usize, &Cursor, &Buffer) -> String,
    bare: impl Fn(&Buffer, &Cursor) -> Option<(usize, usize)>,
) {
    let cursors_before = app.editor.cursors.clone();
    let all = cursors_before.all();
    if all.is_empty() {
        return;
    }

    let mut infos: Vec<(Edit, u32)> = Vec::new();
    for (i, c) in all.iter().enumerate() {
        let buf = &app.editor.buffer;
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

    commit_edit_batch(app, infos, cursors_before);
}

/// Port of `commands_edit_lines.go:perLineEdits` with `dedupe=true` (every
/// caller in this file dedupes — Go's only `dedupe=false` caller,
/// clone-line-up/down, is out of Phase-1 scope).
fn per_line_edits(app: &mut App, build: impl Fn(usize, &Buffer) -> Option<Edit>) {
    let cursors_before = app.editor.cursors.clone();
    let all = cursors_before.all();
    if all.is_empty() {
        return;
    }

    let mut infos: Vec<(Edit, u32)> = Vec::new();
    let mut seen: HashSet<usize> = HashSet::new();
    for c in &all {
        let bp = app.editor.buffer.offset_to_line_col(c.position);
        if !seen.insert(bp.line) {
            continue;
        }
        if let Some(edit) = build(bp.line, &app.editor.buffer) {
            infos.push((edit, c.id));
        }
    }

    commit_edit_batch(app, infos, cursors_before);
}

/// Port of `commands_edit.go:execInsertChar`, generalized to arbitrary text
/// so it doubles as the selection-replacing insert path for bracketed
/// paste (`Msg::Paste`, plan Context: "Bracketed-paste `Msg::Paste` may
/// insert text through the same insert path").
pub fn insert_text(app: &mut App, text: &str) {
    if text.is_empty() {
        return;
    }
    per_cursor_selection_edits(
        app,
        move |_i, _c, _buf| text.to_string(),
        |_buf, c| Some((c.position, c.position)),
    );
}

/// Port of `commands_edit.go:execInsertChar`.
pub fn insert_char(app: &mut App, ch: char) {
    let mut buf = [0u8; 4];
    insert_text(app, ch.encode_utf8(&mut buf));
}

/// Port of `commands_edit.go:execNewline` — the Enter hardcoded fast path
/// (plan Context, "Hardcoded fast paths outside the resolver"): inserts a
/// newline plus the CURRENT line's own leading whitespace, preserving
/// indentation.
pub fn newline(app: &mut App) {
    per_cursor_selection_edits(
        app,
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

/// Port of `commands_edit.go:execDeleteLeft` (Backspace).
pub fn delete_left(app: &mut App) {
    per_cursor_selection_edits(
        app,
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
pub fn delete_right(app: &mut App) {
    per_cursor_selection_edits(
        app,
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
pub fn indent(app: &mut App) {
    per_line_edits(app, |line, buf| {
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
pub fn outdent(app: &mut App) {
    per_line_edits(app, dedent_edit_for_line);
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
/// buffer) untouched, so the journal never runs ahead of the buffer.
pub fn undo(app: &mut App) {
    let Some((step, new_pos)) = app.editor.journal.undo_peek() else {
        return;
    };
    let edits: Vec<AppliedEdit> = step.edits.clone();
    let cursors_before: Vec<Cursor> = step.cursors_before.clone();

    match rune_core::undo::apply_inverse(&app.editor.buffer, &edits) {
        Ok(new_buf) => {
            app.editor.buffer = new_buf;
            app.editor.cursors = CursorSet::new_from(&cursors_before);
            app.editor.journal.move_pos(new_pos);
            app.status_message = None;
        }
        Err(e) => {
            app.status_message = Some(format!("undo failed: {e}"));
        }
    }
}

/// Port of `workspace_undo.go:handleRedo` — mirrors `undo` above: reapply
/// the step forward, commit the position move only on success.
pub fn redo(app: &mut App) {
    let Some((step, new_pos)) = app.editor.journal.redo_peek() else {
        return;
    };
    let edits: Vec<AppliedEdit> = step.edits.clone();
    let cursors_after: Vec<Cursor> = step.cursors_after.clone();

    match rune_core::undo::reapply(&app.editor.buffer, &edits) {
        Ok(new_buf) => {
            app.editor.buffer = new_buf;
            app.editor.cursors = CursorSet::new_from(&cursors_after);
            app.editor.journal.move_pos(new_pos);
            app.status_message = None;
        }
        Err(e) => {
            app.status_message = Some(format!("redo failed: {e}"));
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use rune_core::vfs::Mem;
    use std::sync::Arc;

    fn app_with(content: &str, cursor_offset: usize) -> App {
        let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()));
        app.editor.cursors = CursorSet::new(cursor_offset.min(content.len()));
        app.editor.viewport.set_size(80, 23);
        app
    }

    #[test]
    fn insert_char_moves_caret_past_the_inserted_char() {
        let mut app = app_with("ac", 1);
        insert_char(&mut app, 'b');
        assert_eq!(app.editor.buffer.content(), "abc");
        assert_eq!(app.editor.cursors.primary().position, 2);
        assert_eq!(app.editor.journal.len(), 1);
    }

    #[test]
    fn insert_char_replaces_a_selection() {
        let mut app = app_with("hello world", 0);
        app.editor.cursors = CursorSet::new(0).map(|c| Cursor {
            anchor: 0,
            position: 5,
            ..c
        });
        insert_char(&mut app, 'X');
        assert_eq!(app.editor.buffer.content(), "X world");
        assert_eq!(app.editor.cursors.primary().position, 1);
    }

    #[test]
    fn backspace_deletes_one_rune_never_splitting_a_multibyte_char() {
        let mut app = app_with("a\u{6c49}", "a".len() + '\u{6c49}'.len_utf8());
        delete_left(&mut app);
        assert_eq!(app.editor.buffer.content(), "a");
    }

    #[test]
    fn backspace_at_buffer_start_is_a_no_op() {
        let mut app = app_with("abc", 0);
        delete_left(&mut app);
        assert_eq!(app.editor.buffer.content(), "abc");
        assert_eq!(app.editor.journal.len(), 0);
    }

    #[test]
    fn delete_right_removes_one_rune_forward() {
        let mut app = app_with("abc", 0);
        delete_right(&mut app);
        assert_eq!(app.editor.buffer.content(), "bc");
        assert_eq!(app.editor.cursors.primary().position, 0);
    }

    #[test]
    fn newline_preserves_current_line_indentation() {
        let mut app = app_with("  indented", 10);
        newline(&mut app);
        assert_eq!(app.editor.buffer.content(), "  indented\n  ");
        assert_eq!(app.editor.cursors.primary().position, 13);
    }

    #[test]
    fn indent_inserts_a_leading_tab() {
        let mut app = app_with("hello", 2);
        indent(&mut app);
        assert_eq!(app.editor.buffer.content(), "\thello");
    }

    #[test]
    fn outdent_removes_one_leading_tab() {
        let mut app = app_with("\thello", 3);
        outdent(&mut app);
        assert_eq!(app.editor.buffer.content(), "hello");
    }

    #[test]
    fn outdent_removes_up_to_four_leading_spaces() {
        let mut app = app_with("    hello", 5);
        outdent(&mut app);
        assert_eq!(app.editor.buffer.content(), "hello");
    }

    #[test]
    fn outdent_on_a_line_with_no_indentation_is_a_no_op() {
        let mut app = app_with("hello", 0);
        outdent(&mut app);
        assert_eq!(app.editor.buffer.content(), "hello");
        assert_eq!(app.editor.journal.len(), 0);
    }

    #[test]
    fn undo_then_redo_round_trips_content_and_cursors() {
        let mut app = app_with("hello", 5);
        insert_char(&mut app, '!');
        assert_eq!(app.editor.buffer.content(), "hello!");

        undo(&mut app);
        assert_eq!(app.editor.buffer.content(), "hello");
        assert_eq!(app.editor.cursors.primary().position, 5);

        redo(&mut app);
        assert_eq!(app.editor.buffer.content(), "hello!");
        assert_eq!(app.editor.cursors.primary().position, 6);
    }

    #[test]
    fn undo_with_empty_journal_is_a_no_op() {
        let mut app = app_with("hello", 0);
        undo(&mut app);
        assert_eq!(app.editor.buffer.content(), "hello");
    }

    #[test]
    fn cjk_and_emoji_round_trip_byte_exact_through_undo() {
        let mut app = app_with("汉字 👩‍👩‍👧‍👦", 0);
        let original = app.editor.buffer.content().to_string();
        insert_char(&mut app, 'x');
        undo(&mut app);
        assert_eq!(app.editor.buffer.content(), original);
    }
}
