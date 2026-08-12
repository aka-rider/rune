use std::path::Path;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_nav::{DefRole, Ref, RefKind};

use crate::app::App;
use crate::document::DocumentId;
use crate::messages;
use crate::pane::Pane;
use crate::runtime::Effects;
use crate::workspace;

use super::{NavHistory, Place, PlaceKind};

enum TravelOutcome {
    Landed,
    Refused,
    Dropped,
}

fn live_place(app: &App) -> Place {
    let doc = app.active_doc();
    Place {
        doc: app.active,
        path: doc.file_path.clone(),
        offset: doc.cursors.primary().position,
        kind: PlaceKind::Visited,
    }
}

fn peek_current(history: &NavHistory) -> Option<Place> {
    history.places.get(history.current).cloned()
}

pub fn back(app: &mut App, effects: &mut Effects) {
    let live = live_place(app);
    let Some(mut place) = app.nav_history.back(live) else {
        messages::info(app, "no earlier location");
        return;
    };
    loop {
        match travel_to(app, &place, effects) {
            TravelOutcome::Landed | TravelOutcome::Refused => return,
            TravelOutcome::Dropped => {
                let idx = app.nav_history.index();
                app.nav_history.drop_at(idx);
                match peek_current(&app.nav_history) {
                    Some(next) => place = next,
                    None => {
                        messages::info(app, "no earlier location");
                        return;
                    }
                }
            }
        }
    }
}

pub fn forward(app: &mut App, effects: &mut Effects) {
    loop {
        let Some(place) = app.nav_history.forward() else {
            messages::info(app, "no later location");
            return;
        };
        match travel_to(app, &place, effects) {
            TravelOutcome::Landed | TravelOutcome::Refused => return,
            TravelOutcome::Dropped => {
                let idx = app.nav_history.index();
                app.nav_history.drop_at(idx);
            }
        }
    }
}

fn travel_to(app: &mut App, place: &Place, effects: &mut Effects) -> TravelOutcome {
    let resolved = place
        .path
        .as_deref()
        .and_then(|p| workspace::existing_document_for(app, p))
        .or_else(|| app.doc(place.doc).is_some().then_some(place.doc));

    let id = match resolved {
        Some(id) => id,
        None => {
            let Some(path) = place.path.as_deref() else {
                return TravelOutcome::Dropped;
            };
            let active_before = app.active;
            match workspace::open_path_checked(app, path, effects) {
                Some(id) => id,
                None => {
                    report_refusal(app, path, active_before);
                    return TravelOutcome::Refused;
                }
            }
        }
    };

    land(app, id, place.offset, effects);
    TravelOutcome::Landed
}

fn report_refusal(app: &mut App, path: &Path, active_before: DocumentId) {
    if app.active != active_before {
        let showing = app.active_doc().file_name().to_string();
        messages::warn(
            app,
            format!(
                "could not reopen {} \u{2014} now showing {showing}",
                path.display()
            ),
        );
    }
}

fn land(app: &mut App, id: DocumentId, offset: usize, effects: &mut Effects) {
    let cross_doc = app.active != id;
    if cross_doc {
        workspace::switch_to(app, id);
    }
    app.set_focus_pane(Pane::Editor, effects);
    let Some(doc) = app.doc_mut(id) else {
        return;
    };
    let clamped = clamp_to_char_boundary(&doc.buffer, offset.min(doc.buffer.len()));
    doc.cursors = CursorSet::new(clamped);
    if cross_doc {
        report_cross_doc_landing(app, id, clamped);
    }
}

fn clamp_to_char_boundary(buffer: &Buffer, offset: usize) -> usize {
    let mut offset = offset.min(buffer.len());
    while offset > 0 && !buffer.content().is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn report_cross_doc_landing(app: &mut App, id: DocumentId, offset: usize) {
    let Some(doc) = app.doc(id) else {
        return;
    };
    let name = doc.file_name().to_string();
    match nearest_heading(&doc.catalogue, offset) {
        Some(heading) => messages::info(app, format!("{name} \u{2014} {heading}")),
        None => messages::info(app, name),
    }
}

fn nearest_heading(catalogue: &[Ref], offset: usize) -> Option<String> {
    catalogue
        .iter()
        .filter_map(|r| match &r.kind {
            RefKind::Def {
                role: DefRole::Heading(_),
                name,
            } if r.site.start <= offset => Some((r.site.start, name.clone())),
            _ => None,
        })
        .max_by_key(|(start, _)| *start)
        .map(|(_, name)| name)
}
