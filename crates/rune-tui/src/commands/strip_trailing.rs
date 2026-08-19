use rune_core::buffer::{Edit, trailing_whitespace_edits};
use rune_core::cursor::{Cursor, CursorId};
use rune_core::undo::EditKind;

use crate::app::App;
use crate::commands::edit_core::apply_edit_batch_with_cursors;
use crate::commands::reading;
use crate::document::DocumentId;

pub(crate) fn strip_trailing_whitespace(app: &mut App, id: DocumentId) {
    if crate::merge::is_active_on(app, id) {
        return;
    }
    let Some(doc) = app.doc(id) else { return };
    let deletions = trailing_whitespace_edits(doc.buffer.content());
    if deletions.is_empty() {
        return;
    }
    let cursors_before = doc.cursors.clone();
    let carried = cursors_before.all().to_vec();
    let infos = deletions
        .iter()
        .cloned()
        .map(|edit| (edit, CursorId::FIRST))
        .collect();
    let _ = apply_edit_batch_with_cursors(
        app,
        id,
        infos,
        &cursors_before,
        EditKind::StripTrailingWhitespace,
        move |_, _| {
            carried
                .iter()
                .map(|cursor| Cursor {
                    position: offset_after_deletions(cursor.position, &deletions),
                    anchor: offset_after_deletions(cursor.anchor, &deletions),
                    ..*cursor
                })
                .collect()
        },
    );
}

pub(crate) fn leave_reading_then_strip(app: &mut App, id: DocumentId) {
    reading::leave_reading(app, id);
    strip_trailing_whitespace(app, id);
}

fn offset_after_deletions(offset: usize, deletions: &[Edit]) -> usize {
    let mut removed = 0usize;
    for deletion in deletions {
        if offset <= deletion.start {
            break;
        }
        if offset < deletion.end {
            return deletion.start.saturating_sub(removed);
        }
        removed = removed.saturating_add(deletion.end.saturating_sub(deletion.start));
    }
    offset.saturating_sub(removed)
}

#[cfg(test)]
#[path = "strip_trailing_tests.rs"]
mod tests;
