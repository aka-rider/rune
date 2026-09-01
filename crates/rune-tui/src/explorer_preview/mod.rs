use std::path::Path;

use rune_core::buffer::Buffer;

use crate::app::App;
use crate::document::{Document, DocumentId};
use crate::pane::Pane;
use crate::runtime::{CmdError, Effects};
use crate::workspace;

pub struct Preview {
    pub id: DocumentId,
    pub doc: Document,
}

pub(crate) fn after_cursor_move(app: &mut App, effects: &mut Effects) {
    if app.explorer_find().is_some() {
        return;
    }
    let Some(entry) = app.explorer.entries.get(app.explorer.nav.cursor) else {
        return;
    };
    if entry.kind == rune_vfs::FileKind::Dir || entry.link == rune_vfs::Link::Broken {
        return;
    }
    let target = entry.path.clone();
    request_preview(app, &target, effects);
}

pub(crate) fn request_preview(app: &mut App, target: &Path, effects: &mut Effects) {
    let Some(resolved) = resolved_target(app, target) else {
        return;
    };
    if let Some(id) = workspace::existing_document_for(app, &resolved) {
        workspace::switch_to(app, id);
        return;
    }
    let target = resolved.as_path();

    let already_showing = shown_path(app) == Some(target);
    let already_failed = app.explorer.preview_failed.as_deref() == Some(target);
    let already_awaiting = app.explorer.preview_awaiting.as_deref() == Some(target);
    if already_showing || already_failed || already_awaiting {
        return;
    }

    let generation = app.explorer.mint_preview_generation();
    app.explorer.preview_generation = generation;
    app.explorer.preview_awaiting = Some(target.to_path_buf());
    let vfs = std::sync::Arc::clone(&app.vfs);
    effects.cmds.push(crate::runtime::read_preview_cmd(
        vfs,
        target.to_path_buf(),
        generation,
    ));
}

pub(crate) fn shown_path(app: &App) -> Option<&Path> {
    app.explorer
        .preview
        .as_ref()
        .and_then(|preview| preview.doc.path())
}

/// A `Msg::FileOpened` reply belongs to the live preview only when its
/// `preview_generation` echoes what `request_preview` minted — never a path
/// match. A real file open always carries `preview_generation: None`, so it
/// can never be mistaken for a stale preview reply.
pub(crate) fn maybe_consume_reply(
    app: &mut App,
    path: &Path,
    preview_generation: Option<crate::generation::PreviewGen>,
    result: &Result<Vec<u8>, CmdError>,
    effects: &mut Effects,
) -> bool {
    let Some(generation) = preview_generation else {
        return false;
    };
    if generation != app.explorer.preview_generation {
        return true;
    }
    app.explorer.preview_awaiting = None;
    if workspace::existing_document_for_spelling(app, path).is_some() {
        return true;
    }
    match result {
        Ok(bytes) => apply_loaded(app, path, bytes.clone(), effects),
        Err(reason) => apply_failed(app, path, &reason.to_string(), effects),
    }
    true
}

fn resolved_target(app: &App, path: &Path) -> Option<crate::resolved::ResolvedPath> {
    crate::resolved::ResolvedPath::resolve(app.vfs.as_ref(), path).ok()
}

fn apply_loaded(app: &mut App, path: &Path, bytes: Vec<u8>, effects: &mut Effects) {
    let Ok(buffer) = Buffer::from_bytes(bytes) else {
        apply_failed(app, path, "not valid UTF-8", effects);
        return;
    };
    let Some(resolved) = resolved_target(app, path) else {
        apply_failed(app, path, "could not resolve this path", effects);
        return;
    };
    app.explorer.preview_failed = None;
    install(app, Document::new_bound(buffer, resolved), effects);
}

fn apply_failed(app: &mut App, path: &Path, reason: &str, effects: &mut Effects) {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("this file");
    let text = format!("cannot preview {file_name} — {reason}");
    app.explorer.preview_failed = Some(path.to_path_buf());
    let mut doc = Document::new(Buffer::new(text));
    doc.display_name = Some(file_name.to_string());
    install(app, doc, effects);
}

fn install(app: &mut App, mut doc: Document, effects: &mut Effects) {
    let (width, height) = app.editor_viewport_size();
    doc.viewport.set_size(width, height);
    let id = app.mint_doc_id();
    app.explorer.preview = Some(Preview { id, doc });
    crate::highlight::schedule_highlight(app, id, effects);
}

pub(crate) fn discard(app: &mut App) {
    app.explorer.preview = None;
    app.explorer.preview_failed = None;
    app.explorer.preview_generation = app.explorer.mint_preview_generation();
    app.explorer.preview_awaiting = None;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Promotion {
    Promoted(DocumentId),
    NothingToPromote,
    Refused,
}

pub(crate) fn on_focus_changed(
    app: &mut App,
    previous: Pane,
    current: Pane,
    effects: &mut Effects,
) {
    if previous == current {
        return;
    }
    match current {
        Pane::Editor => {
            editor_takes_over(app, effects);
        }
        Pane::Title | Pane::Tabs => discard(app),
        Pane::Explorer | Pane::Messages => {}
    }
}

pub(crate) fn promote(app: &mut App, effects: &mut Effects) -> Promotion {
    let Some(preview) = app.explorer.preview.take() else {
        return Promotion::NothingToPromote;
    };
    let Some(path) = preview.doc.resolved_path().cloned() else {
        return Promotion::NothingToPromote;
    };
    if !crate::opentabs::limit::ensure_room(app, effects) {
        app.explorer.preview = Some(preview);
        return Promotion::Refused;
    }
    app.explorer.preview_failed = None;
    let departed = crate::navhistory::departure_origin(app);
    let id = preview.id;
    app.documents.insert(id, preview.doc);
    let _ =
        crate::db_enqueue::load_document(app, id, &path, crate::db_enqueue::LoadIntent::Recover);
    workspace::switch_to(app, id);
    crate::navhistory::record_departure_if_moved(app, departed);
    Promotion::Promoted(id)
}

pub(crate) fn editor_takes_over(app: &mut App, effects: &mut Effects) -> Promotion {
    let outcome = promote(app, effects);
    if outcome == Promotion::Refused {
        discard(app);
    }
    outcome
}

#[cfg(test)]
mod tests_common;
#[cfg(test)]
mod tests_focus;
#[cfg(test)]
mod tests_highlight;
#[cfg(test)]
mod tests_identity;
#[cfg(test)]
mod tests_paint;
#[cfg(test)]
mod tests_preview;
#[cfg(test)]
mod tests_retry;
