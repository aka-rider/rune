use rune_core::buffer::{AppliedEdit, Buffer, Edit};
use rune_core::cursor::{Cursor, CursorId, CursorSet};
use rune_core::undo::{EditKind, Journal};

use crate::app::App;
use crate::commands::edit_core::commit_edit_batch;
use crate::commands::nav;
use crate::commands::nav_line;
use crate::db_enqueue as db;
use crate::document::{DocumentId, ReadOnly};
use crate::messages;
use crate::undogroup::{self, Direction, Tier};

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
                start: start.get(),
                end: end.get(),
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

pub fn insert_text(app: &mut App, id: DocumentId, text: &str, kind: EditKind) {
    if text.is_empty() {
        return;
    }
    per_cursor_selection_edits(
        app,
        id,
        kind,
        move |_i, _c, _buf| text.to_string(),
        |_buf, c| Some((c.position.get(), c.position.get())),
    );
}

pub fn insert_char(app: &mut App, id: DocumentId, ch: char) {
    let mut buf = [0u8; 4];
    insert_text(app, id, ch.encode_utf8(&mut buf), EditKind::Insert);
}

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
            let bp = buf.offset_to_line_col(pos.get());
            let line = buf.line(bp.line);
            let indent: String = line
                .chars()
                .take_while(|&ch| ch == ' ' || ch == '\t')
                .collect();
            format!("\n{indent}")
        },
        |_buf, c| Some((c.position.get(), c.position.get())),
    );
}

pub(crate) fn delete_selection_or_line(app: &mut App, id: DocumentId) {
    per_cursor_selection_edits(
        app,
        id,
        EditKind::Cut,
        |_i, _c, _buf| String::new(),
        |buf, c| Some(nav_line::line_range_incl_newline(buf, c.position.get())),
    );
}

pub fn delete_left(app: &mut App, id: DocumentId) {
    per_cursor_selection_edits(
        app,
        id,
        EditKind::DeleteLeft,
        |_i, _c, _buf| String::new(),
        |buf, c| {
            if c.position.get() == 0 {
                None
            } else {
                Some((
                    nav::prev_rune_offset(buf, c.position.get()),
                    c.position.get(),
                ))
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
            if c.position.get() >= buf.len() {
                None
            } else {
                Some((
                    c.position.get(),
                    nav::next_rune_offset(buf, c.position.get()),
                ))
            }
        },
    );
}

pub fn delete_word_left(app: &mut App, id: DocumentId) {
    per_cursor_selection_edits(
        app,
        id,
        EditKind::DeleteLeft,
        |_i, _c, _buf| String::new(),
        |buf, c| {
            if c.position.get() == 0 {
                None
            } else {
                Some((
                    nav::word_left_offset(buf, c.position.get()),
                    c.position.get(),
                ))
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
            if c.position.get() >= buf.len() {
                None
            } else {
                Some((
                    c.position.get(),
                    nav::word_right_offset(buf, c.position.get()),
                ))
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
        let _ = crate::db_enqueue::probe(app, id);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
#[path = "edit_tests.rs"]
mod tests;
