use std::path::{Path, PathBuf};

use rune_core::buffer::Buffer;

use crate::app::App;
use crate::document::{Document, DocumentId, ReadOnly};
use crate::pane::Pane;
use crate::runtime::{CmdError, Effects};
use crate::workspace;

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
    let target = resolved.as_path();
    if let Some(id) = workspace::existing_document_for(app, target) {
        workspace::switch_to(app, id);
        return;
    }

    let already_showing = app
        .explorer
        .preview
        .and_then(|id| app.doc(id))
        .and_then(|doc| doc.file_path.as_deref())
        == Some(target);
    let already_failed = app.explorer.preview_failed.as_deref() == Some(target);
    if already_showing || already_failed || app.explorer.preview_awaiting.contains(target) {
        return;
    }

    app.explorer.preview_awaiting.insert(target.to_path_buf());
    let vfs = std::sync::Arc::clone(&app.vfs);
    effects
        .cmds
        .push(crate::runtime::read_preview_cmd(vfs, target.to_path_buf()));
}

pub(crate) fn maybe_consume_reply(
    app: &mut App,
    path: &Path,
    result: &Result<Vec<u8>, CmdError>,
) -> bool {
    if !app.explorer.preview_awaiting.remove(path) {
        return false;
    }
    if workspace::existing_document_for(app, path).is_some() {
        return true;
    }
    if !is_current_target(app, path) {
        return true;
    }
    match result {
        Ok(bytes) => apply_loaded(app, path, bytes.clone()),
        Err(reason) => apply_failed(app, path, &reason.to_string()),
    }
    true
}

fn is_current_target(app: &App, path: &Path) -> bool {
    if app.filesearch().is_some() {
        return crate::filesearch::selected_candidate(app)
            .and_then(|c| resolved_target(app, &c.path))
            .is_some_and(|selected| selected == path);
    }
    if app.explorer_find().is_some() {
        return false;
    }
    app.explorer
        .entries
        .get(app.explorer.nav.cursor)
        .filter(|e| e.kind != rune_vfs::FileKind::Dir)
        .and_then(|e| resolved_target(app, &e.path))
        .is_some_and(|selected| selected == path)
}

fn resolved_target(app: &App, path: &Path) -> Option<PathBuf> {
    workspace::resolve(app.vfs.as_ref(), path).ok()
}

fn apply_loaded(app: &mut App, path: &Path, bytes: Vec<u8>) {
    let Ok(buffer) = Buffer::from_bytes(bytes) else {
        apply_failed(app, path, "not valid UTF-8");
        return;
    };
    app.explorer.preview_failed = None;
    let id = match app.explorer.preview.filter(|id| app.doc(*id).is_some()) {
        Some(id) => {
            if app.merge.doc() == Some(id) {
                crate::merge::auto_exit(app);
            }
            let floor = app.doc(id).map_or(0, |doc| doc.buffer.version());
            let buffer = buffer.advance_past(floor);
            app.nav_history.drop_doc(id);
            if let Some(doc) = app.doc_mut(id) {
                *doc = Document::new(buffer);
                doc.bind_path(path.to_path_buf());
                doc.read_only = ReadOnly::Preview;
            }
            id
        }
        None => {
            if app.documents.order().len() >= crate::opentabs::limit::MAX_TABS {
                return;
            }
            let id = app.open_document(buffer);
            if let Some(doc) = app.doc_mut(id) {
                doc.bind_path(path.to_path_buf());
                doc.read_only = ReadOnly::Preview;
            }
            app.explorer.preview = Some(id);
            id
        }
    };
    workspace::switch_to(app, id);
}

fn apply_failed(app: &mut App, path: &Path, reason: &str) {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("this file");
    let text = format!("cannot preview {file_name} — {reason}");
    app.explorer.preview_failed = Some(path.to_path_buf());
    let id = match app.explorer.preview.filter(|id| app.doc(*id).is_some()) {
        Some(id) => {
            let floor = app.doc(id).map_or(0, |doc| doc.buffer.version());
            let buffer = Buffer::new(text).advance_past(floor);
            app.nav_history.drop_doc(id);
            if let Some(doc) = app.doc_mut(id) {
                *doc = Document::new(buffer);
                doc.read_only = ReadOnly::Preview;
                doc.display_name = Some(file_name.to_string());
            }
            id
        }
        None => {
            if app.documents.order().len() >= crate::opentabs::limit::MAX_TABS {
                return;
            }
            let id = app.open_document(Buffer::new(text));
            if let Some(doc) = app.doc_mut(id) {
                doc.read_only = ReadOnly::Preview;
                doc.display_name = Some(file_name.to_string());
            }
            app.explorer.preview = Some(id);
            id
        }
    };
    workspace::switch_to(app, id);
}

pub(crate) fn discard_if_switching_away(app: &mut App, target: DocumentId) {
    let Some(id) = app.explorer.preview else {
        return;
    };
    if id == target {
        return;
    }
    remove_preview_document(app, id);
}

pub(crate) fn on_focus_changed(app: &mut App, previous: Pane, current: Pane) {
    if previous == current {
        return;
    }
    match current {
        Pane::Editor => {
            if let Some(id) = app.explorer.preview {
                promote(app, id);
            }
        }
        Pane::Title | Pane::Tabs => discard_active(app),
        Pane::Explorer | Pane::Messages => {}
    }
}

pub(crate) fn promote(app: &mut App, id: DocumentId) {
    if app.explorer.preview != Some(id) {
        return;
    }
    if let Some(doc) = app.doc_mut(id) {
        doc.read_only = ReadOnly::No;
    }
    app.explorer.preview = None;
    let path = app.doc(id).and_then(|doc| doc.file_path.clone());
    if let Some(path) = path {
        let _ = crate::db_enqueue::load_document(
            app,
            id,
            &path,
            crate::db_enqueue::LoadIntent::Recover,
        );
    }
}

fn discard_active(app: &mut App) {
    let Some(id) = app.explorer.preview else {
        return;
    };
    let was_active = app.active == id;
    let target = was_active
        .then(|| {
            app.explorer
                .browsing_origin
                .live_excluding(app, id)
                .or_else(|| workspace::close::neighbor_of(app, id))
        })
        .flatten();
    remove_preview_document(app, id);
    if let Some(target) = target {
        workspace::switch_to(app, target);
    }
    app.tabs.nav.cursor = app
        .documents
        .order()
        .iter()
        .position(|&t| t == app.active)
        .unwrap_or(0);
}

fn remove_preview_document(app: &mut App, id: DocumentId) {
    app.documents.remove(&id);
    app.explorer.preview = None;
    app.explorer.preview_failed = None;
}

#[cfg(test)]
mod tests_common;
#[cfg(test)]
mod tests_focus;
#[cfg(test)]
mod tests_highlight;
#[cfg(test)]
mod tests_preview;
#[cfg(test)]
mod tests_retry;
