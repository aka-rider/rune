use crate::app::App;
use crate::document::DocumentId;

use super::{NavHistory, Place, PlaceKind};

const NAV_JUMP_LINES: usize = 10;

fn eligible(app: &App, id: DocumentId) -> bool {
    match app.doc(id) {
        Some(doc) if doc.is_preview() => false,
        Some(_) => app.help_doc != Some(id),
        None => false,
    }
}

fn place_for(app: &App, doc: DocumentId, offset: usize, kind: PlaceKind) -> Option<Place> {
    let path = app.doc(doc)?.file_path.clone();
    Some(Place {
        doc,
        path,
        offset,
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

pub fn observe(app: &mut App, active_before: DocumentId, before: &[(DocumentId, usize)]) {
    if app.active != active_before {
        observe_switch(app, active_before, before);
    } else {
        observe_same_doc(app, before);
    }
}

fn observe_switch(app: &mut App, outgoing: DocumentId, before: &[(DocumentId, usize)]) {
    if !eligible(app, outgoing) || !eligible(app, app.active) {
        return;
    }
    let new_caret = app.active_doc().cursors.primary().position;
    let before_caret = before
        .iter()
        .find(|(d, _)| *d == app.active)
        .map(|&(_, p)| p);
    if before_caret == Some(new_caret) {
        return;
    }
    let Some(offset) = before.iter().find(|(d, _)| *d == outgoing).map(|&(_, p)| p) else {
        return;
    };
    let Some(place) = place_for(app, outgoing, offset, PlaceKind::Visited) else {
        return;
    };
    push_with_line_dedup(app, place);
}

fn observe_same_doc(app: &mut App, before: &[(DocumentId, usize)]) {
    let id = app.active;
    if !eligible(app, id) {
        return;
    }
    let Some(&(_, before_offset)) = before.iter().find(|(d, _)| *d == id) else {
        return;
    };
    let after_offset = app.active_doc().cursors.primary().position;
    if before_offset == after_offset {
        return;
    }
    let buffer = &app.active_doc().buffer;
    let before_line = buffer.offset_to_line_col(before_offset).line;
    let after_line = buffer.offset_to_line_col(after_offset).line;
    if before_line.abs_diff(after_line) < NAV_JUMP_LINES {
        return;
    }
    let Some(place) = place_for(app, id, before_offset, PlaceKind::Visited) else {
        return;
    };
    push_with_line_dedup(app, place);
}
