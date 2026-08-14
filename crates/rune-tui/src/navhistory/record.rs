use crate::app::App;
use crate::document::DocumentId;
use crate::focus::{self, FocusTarget};

use super::{NavHistory, Place, PlaceKind, clamp_to_char_boundary};

const NAV_JUMP_LINES: usize = 10;

fn eligible(app: &App, id: DocumentId) -> bool {
    match app.doc(id) {
        Some(doc) if doc.is_preview() => false,
        Some(_) => app.help_doc != Some(id),
        None => false,
    }
}

fn place_for(app: &App, doc: DocumentId, offset: usize, kind: PlaceKind) -> Option<Place> {
    let document = app.doc(doc)?;
    Some(Place {
        doc,
        path: document.file_path.clone(),
        offset: clamp_to_char_boundary(&document.buffer, offset),
        kind,
    })
}

fn top_entry(history: &NavHistory) -> Option<&Place> {
    history
        .current
        .checked_sub(1)
        .and_then(|i| history.places.get(i))
}

fn same_doc_and_line(app: &App, a: &Place, b: &Place) -> bool {
    if a.doc != b.doc {
        return false;
    }
    let Some(doc) = app.doc(a.doc) else {
        return false;
    };
    doc.buffer.offset_to_line_col(a.offset).line == doc.buffer.offset_to_line_col(b.offset).line
}

fn push_with_line_dedup(app: &mut App, place: Place) {
    let dedup = top_entry(&app.nav_history).is_some_and(|top| same_doc_and_line(app, top, &place));
    app.nav_history.push(place, dedup);
}

pub fn record_edit(app: &mut App, id: DocumentId, offset: usize) {
    let Some(place) = place_for(app, id, offset, PlaceKind::Edited) else {
        return;
    };
    let replace = top_entry(&app.nav_history)
        .is_some_and(|top| top.doc == id && top.kind == PlaceKind::Edited);
    app.nav_history.push(place, replace);
}

/// Where a navigation is departing FROM, read before that navigation moves
/// anything: the browsing origin whenever the user is browsing the
/// Explorer — `app.active` names an ineligible preview there, or the
/// destination itself once the cursor lands on a file already open — and
/// the active document otherwise.
pub fn departure_origin(app: &App) -> Option<DocumentId> {
    let browsing =
        focus::target(app) == FocusTarget::Explorer || app.explorer.preview == Some(app.active);
    if browsing {
        app.explorer.browsing_origin
    } else {
        Some(app.active)
    }
}

pub fn record_departure(app: &mut App, from: DocumentId) {
    if !eligible(app, from) {
        return;
    }
    let Some(offset) = app.doc(from).map(|doc| doc.cursors.primary().position) else {
        return;
    };
    let Some(place) = place_for(app, from, offset, PlaceKind::Visited) else {
        return;
    };
    push_with_line_dedup(app, place);
}

pub fn record_departure_if_moved(app: &mut App, from: Option<DocumentId>) {
    let Some(from) = from.filter(|&id| id != app.active) else {
        return;
    };
    record_departure(app, from);
}

pub fn observe_jump(app: &mut App, active_before: DocumentId, caret_before: usize) {
    if app.active != active_before || !eligible(app, active_before) {
        return;
    }
    let after_offset = app.active_doc().cursors.primary().position;
    if caret_before == after_offset {
        return;
    }
    let buffer = &app.active_doc().buffer;
    let before_line = buffer.offset_to_line_col(caret_before).line;
    let after_line = buffer.offset_to_line_col(after_offset).line;
    if before_line.abs_diff(after_line) < NAV_JUMP_LINES {
        return;
    }
    let Some(place) = place_for(app, active_before, caret_before, PlaceKind::Visited) else {
        return;
    };
    push_with_line_dedup(app, place);
}
