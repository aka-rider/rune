use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::app::App;
use crate::commands::mouse::select_range;
use crate::document::DocumentId;
use crate::find::project::MIN_QUERY_CHARS;
use crate::find::{follow, navigating_as_panel};
use crate::runtime::Effects;
use crate::viewport::ScrollMode;

// A Workspace keystroke lands on the origin document's first hit at or after
// the origin cursor, with no wrap-around: a hit before the cursor yields to
// the project reply, which may name a file further on.
pub(crate) fn follow_workspace(app: &mut App) {
    let Some(state) = app.find() else {
        return;
    };
    let Some(origin_doc) = app.doc(state.origin.doc) else {
        return;
    };
    let from = state.origin.cursors.primary().selection_start().get();
    let scanned;
    let hits: &[Range<usize>] = if state.doc == state.origin.doc {
        &state.matches
    } else {
        scanned = state
            .pattern
            .as_ref()
            .map(|matcher| matcher.hits(origin_doc.buffer.content()))
            .unwrap_or_default();
        &scanned
    };
    let concealed = follow::concealed_in(origin_doc);
    let found = follow::at_or_after(hits, from, |m| follow::is_concealed(&concealed, m))
        .filter(|&idx| hits.get(idx).is_some_and(|m| m.start >= from));
    let below_minimum = state.find.editor.text().chars().count() < MIN_QUERY_CHARS;
    match found {
        Some(idx) => land_in_origin(app, idx),
        None => no_hit_in_origin(app, below_minimum),
    }
}

fn land_in_origin(app: &mut App, idx: usize) {
    show_origin(app);
    let Some(range) = app.find().and_then(|state| state.matches.get(idx).cloned()) else {
        return;
    };
    follow::select_match(app, range);
    if let Some(state) = app.find_mut() {
        state.current = Some(idx);
        if let Some(project) = state.project_mut() {
            project.landed_in_origin = true;
        }
    }
}

// Past the minimum length the project reply decides what is shown next, so
// whatever is on screen stays until it arrives rather than flashing the
// origin in between keystrokes.
fn no_hit_in_origin(app: &mut App, below_minimum: bool) {
    if let Some(project) = app.find_mut().and_then(|state| state.project_mut()) {
        project.landed_in_origin = false;
    }
    follow::restore_to_origin(app);
    if below_minimum {
        show_origin(app);
    }
}

pub(crate) fn show_origin(app: &mut App) {
    let Some(origin) = app.find().map(|state| state.origin.doc) else {
        return;
    };
    if app.active != origin {
        navigating_as_panel(app, |app| crate::workspace::switch_to(app, origin));
    }
    crate::explorer_preview::discard(app);
    if let Some(project) = app.find_mut().and_then(|state| state.project_mut()) {
        project.pending_center = None;
    }
    crate::find::sync(app);
}

pub(crate) fn settle_without_results(app: &mut App) {
    let landed = app
        .find()
        .and_then(|state| state.project())
        .is_some_and(|project| project.landed_in_origin);
    if !landed {
        show_origin(app);
        follow::restore_to_origin(app);
    }
}

// A panel the user has already left (a click into the editor accepted the
// location) never navigates again: the editor owns the screen and the keys.
pub(crate) fn on_reply(app: &mut App, effects: &mut Effects) {
    let Some(state) = app.find() else {
        return;
    };
    if !state.focused {
        return;
    }
    let Some(project) = state.project() else {
        return;
    };
    if project.landed_in_origin {
        select_origin_entry(app);
    } else if project.results.is_empty() {
        settle_without_results(app);
    } else {
        focus_selected_hit(app, effects);
    }
}

fn select_origin_entry(app: &mut App) {
    let Some(path) = app.active_doc().path().map(Path::to_path_buf) else {
        return;
    };
    if let Some(project) = app.find_mut().and_then(|state| state.project_mut())
        && let Some(at) = project.results.iter().position(|hit| hit.path == path)
    {
        project.list.cursor = at;
    }
}

pub(crate) fn selected_hit(app: &App) -> Option<(PathBuf, Range<usize>)> {
    let hit = app.find()?.project()?.selected()?;
    let range = hit
        .ranges
        .first()
        .cloned()
        .unwrap_or(hit.first_match..hit.first_match);
    Some((hit.path.clone(), range))
}

// The match the user already sees selected in the active document outranks
// the file's first match when the hit is opened.
pub(crate) fn shown_match(app: &App, path: &Path) -> Option<Range<usize>> {
    let state = app.find()?;
    let current = state.current?;
    (state.doc == app.active && app.active_doc().path() == Some(path))
        .then(|| state.matches.get(current).cloned())?
}

pub(crate) fn focus_selected_hit(app: &mut App, effects: &mut Effects) {
    let Some((path, range)) = selected_hit(app) else {
        return;
    };
    if crate::workspace::existing_document_for_spelling(app, &path) == Some(app.active) {
        land_at(app, app.active, range);
        return;
    }
    navigating_as_panel(app, |app| {
        crate::explorer_preview::request_preview(app, &path, effects);
    });
    if let Some(project) = app.find_mut().and_then(|state| state.project_mut()) {
        project.pending_center = Some((path, range.start));
    }
    apply_pending_center(app);
}

pub(crate) fn apply_pending_center(app: &mut App) {
    let Some((path, offset)) = app
        .find()
        .and_then(|state| state.project())
        .and_then(|project| project.pending_center.clone())
    else {
        return;
    };
    let Some(target) = crate::workspace::shown_document_for(app, &path) else {
        return;
    };
    if let Some(doc) = app.live_doc_mut(target) {
        crate::commands::nav_scroll::centre_on_byte_offset(doc, offset);
    }
    if let Some(project) = app.find_mut().and_then(|state| state.project_mut()) {
        project.pending_center = None;
    }
}

pub(crate) fn land_at(app: &mut App, id: DocumentId, range: Range<usize>) {
    let Some(doc) = app.doc_mut(id) else {
        return;
    };
    let len = doc.buffer.content().len();
    let start = range.start.min(len);
    let end = range.end.min(len);
    select_range(doc, start, end);
    doc.viewport.mode = ScrollMode::EnsureVisible;
    crate::find::sync(app);
    if let Some(state) = app.find_mut() {
        state.current = state.matches.iter().position(|m| m.start == start);
    }
}
