use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_core::buffer::{Buffer, BufferError};
use rune_vfs::Vfs;

use crate::app::App;
use crate::db_enqueue as db;
use crate::document::{DocumentId, ReadOnly};
use crate::graphics::{ImageState, ImageStatus};
use crate::help;
use crate::messages;
use crate::pane::Pane;
use crate::runtime::{CmdError, Effects};

pub fn resolve(vfs: &dyn Vfs, path: &Path) -> std::io::Result<PathBuf> {
    vfs.resolve(path)
}

pub fn resolve_or_report(app: &mut App, path: &Path, verb: &str) -> Option<PathBuf> {
    match resolve(app.vfs.as_ref(), path) {
        Ok(resolved) => Some(resolved),
        Err(e) => {
            messages::error(app, format!("could not {verb} {}: {e}", path.display()));
            None
        }
    }
}

pub fn open_path(app: &mut App, path: &Path) -> Option<DocumentId> {
    match resolve_and_read(app, path) {
        ReadOutcome::Reactivated(id) => Some(id),
        ReadOutcome::Read { resolved, bytes } => open_bytes(app, &resolved, bytes),
        ReadOutcome::Failed => None,
    }
}

pub fn open_path_checked(app: &mut App, path: &Path, effects: &mut Effects) -> Option<DocumentId> {
    match resolve_and_read(app, path) {
        ReadOutcome::Reactivated(id) => Some(id),
        ReadOutcome::Read { resolved, bytes } => {
            if !crate::opentabs::limit::ensure_room(app, effects) {
                return None;
            }
            open_bytes(app, &resolved, bytes)
        }
        ReadOutcome::Failed => None,
    }
}

enum ReadOutcome {
    Reactivated(DocumentId),
    Read { resolved: PathBuf, bytes: Vec<u8> },
    Failed,
}

fn resolve_and_read(app: &mut App, path: &Path) -> ReadOutcome {
    let Some(resolved) = resolve_or_report(app, path, "open") else {
        return ReadOutcome::Failed;
    };

    if let Some(id) = existing_document_for(app, &resolved) {
        switch_to(app, id);
        return ReadOutcome::Reactivated(id);
    }

    match rune_vfs::get(
        app.vfs.as_ref(),
        &resolved,
        Some(rune_vfs::MAX_DOCUMENT_BYTES),
    ) {
        Ok(sighting) => ReadOutcome::Read {
            resolved,
            bytes: sighting.bytes,
        },
        Err(e) => {
            messages::error(app, format!("could not open {}: {e}", resolved.display()));
            ReadOutcome::Failed
        }
    }
}

pub fn open_path_async(
    app: &mut App,
    path: &Path,
    anchor: Option<rune_nav::Anchor>,
    effects: &mut Effects,
) {
    let Some(resolved) = resolve_or_report(app, path, "open") else {
        return;
    };

    if let Some(id) = existing_document_for(app, &resolved) {
        app.blur_title(effects);
        switch_to(app, id);
        app.set_focus_pane(Pane::Editor, effects);
        if let Some(anchor) = anchor {
            crate::navigate::land_anchor(app, id, &anchor);
        }
        return;
    }

    let vfs = Arc::clone(&app.vfs);
    effects
        .cmds
        .push(crate::runtime::read_file_cmd(vfs, resolved, anchor));
}

pub(crate) fn handle_file_opened(
    app: &mut App,
    path: &Path,
    result: Result<Vec<u8>, CmdError>,
    anchor: Option<rune_nav::Anchor>,
    preview_generation: Option<crate::generation::PreviewGen>,
    effects: &mut Effects,
) {
    if crate::explorer_preview::maybe_consume_reply(app, path, preview_generation, &result) {
        return;
    }

    app.blur_title(effects);

    if let Some(id) = existing_document_for(app, path) {
        switch_to(app, id);
        app.set_focus_pane(Pane::Editor, effects);
        if let Some(anchor) = anchor {
            crate::navigate::land_anchor(app, id, &anchor);
        }
        return;
    }

    let bytes = match result {
        Ok(bytes) => bytes,
        Err(e) => {
            messages::error(app, format!("could not open {}: {e}", path.display()));
            return;
        }
    };
    if !crate::opentabs::limit::ensure_room(app, effects) {
        return;
    }
    let Some(id) = open_bytes(app, path, bytes) else {
        return;
    };
    app.set_focus_pane(Pane::Editor, effects);
    if let Some(anchor) = anchor {
        crate::navigate::land_anchor(app, id, &anchor);
    }
}

fn open_bytes(app: &mut App, resolved: &Path, bytes: Vec<u8>) -> Option<DocumentId> {
    if crate::document_support::is_image_path(resolved) {
        return Some(open_image_bytes(app, resolved, &bytes));
    }

    let buffer = match Buffer::from_bytes(bytes) {
        Ok(buffer) => buffer,
        Err(BufferError::InvalidUtf8) => {
            messages::error(
                app,
                format!(
                    "{}: not valid UTF-8 \u{2014} refusing to open",
                    resolved.display()
                ),
            );
            return None;
        }
        Err(e) => {
            messages::error(app, format!("could not open {}: {e}", resolved.display()));
            return None;
        }
    };

    let id = app.open_document(buffer);
    if let Some(doc) = app.doc_mut(id) {
        doc.bind_path(resolved.to_path_buf());
    }
    let _ = db::load_document(app, id, resolved, db::LoadIntent::Recover);
    switch_to(app, id);
    Some(id)
}

fn open_image_bytes(app: &mut App, resolved: &Path, bytes: &[u8]) -> DocumentId {
    let dims = rune_image::probe_dimensions(bytes).map(|(w, h, _)| rune_image::PixelSize { w, h });
    // The whole-document image path and the inline-embed path share ONE
    // terminal-global allocator (`App::image_ids`) keyed by this same
    // resolved-path string — Kitty image ids are terminal-global, so a
    // per-document hash alone (the old `rune_image::alloc_id` call this
    // replaced) could hand two different open documents the same id.
    let key = resolved.to_string_lossy().into_owned();
    let id = app.image_ids.alloc_free_id(&key);
    let bytes_len = bytes.len() as u64;
    let file_name = resolved
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("image")
        .to_string();

    let doc_id = app.open_document(Buffer::new(""));
    if let Some(doc) = app.doc_mut(doc_id) {
        doc.bind_path(resolved.to_path_buf());
        doc.read_only = ReadOnly::Always;
        doc.display_name = Some(file_name);
        doc.set_image(ImageState {
            path: resolved.to_path_buf(),
            bytes_len,
            id,
            dims,
            status: ImageStatus::Pending,
            in_flight: None,
            next_generation: crate::generation::GenCounter::default(),
        });
    }
    switch_to(app, doc_id);
    doc_id
}

pub(crate) fn existing_document_for(app: &App, path: &Path) -> Option<DocumentId> {
    app.documents
        .iter()
        .find(|(_, doc)| doc.file_path.as_deref() == Some(path))
        .map(|(id, _)| *id)
}

pub fn switch_to(app: &mut App, id: DocumentId) {
    if app.doc(id).is_none() {
        return;
    }
    crate::explorer_preview::discard_if_switching_away(app, id);
    if app.merge.doc().is_some_and(|merge_doc| merge_doc != id) {
        crate::merge::auto_exit(app);
    }
    app.active = id;
    app.documents.touch(id);
    let name = crate::title::name_for(app.active_doc());
    app.title.seed(&name);
    if let Some(idx) = app.documents.order().iter().position(|&t| t == id) {
        app.tabs.nav.cursor = idx;
    }
    crate::db_enqueue::probe(app, id);
}

/// Activates the tab at `idx`, or does nothing and reports `false` when no
/// tab is open at that position — the caller decides whether that's worth
/// telling the user about (`pane_global::tab_switch` does; `opentabs::
/// activate` never passes an out-of-range index in the first place, since
/// its own cursor is already clamped to the open tab count).
pub fn select_tab(app: &mut App, idx: usize) -> bool {
    let Some(&id) = app.documents.order().get(idx) else {
        return false;
    };
    let departed = crate::navhistory::departure_origin(app);
    switch_to(app, id);
    crate::navhistory::record_departure_if_moved(app, departed);
    true
}

pub fn toggle_help(app: &mut App, effects: &mut Effects) {
    let live_help = app.help_doc.filter(|id| app.documents.contains_key(id));

    if let Some(id) = live_help {
        if app.active == id {
            let target = app
                .help_return_to
                .live_excluding(app, id)
                .or_else(|| app.documents.keys().find(|&&other| other != id).copied())
                .unwrap_or(id);
            switch_to(app, target);
        } else {
            app.help_return_to = crate::returnto::ReturnTo::to(app.active);
            switch_to(app, id);
        }
        return;
    }

    if !crate::opentabs::limit::ensure_room(app, effects) {
        return;
    }

    let previous = app.active;
    let id = app.open_document(Buffer::new(help::help_markdown()));
    if let Some(doc) = app.doc_mut(id) {
        doc.read_only = ReadOnly::Always;
        doc.display_name = Some("Help".to_string());
    }
    app.help_doc = Some(id);
    app.help_return_to = crate::returnto::ReturnTo::to(previous);
    switch_to(app, id);
}

pub(crate) mod close;
pub use close::{
    CloseOutcome, close_now, new_untitled_document, next_untitled_name, request_close,
};

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use rune_core::buffer::Buffer as CoreBuffer;
    use rune_vfs::{Mem, Vfs, VfsTestExt};
    use std::sync::Arc;

    fn app_with_seed(mem: &Arc<Mem>) -> App {
        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
        App::new(CoreBuffer::new(""), None, vfs, None)
    }

    #[test]
    fn opening_a_new_path_inserts_a_document_with_no_recovery_binding() {
        let mem = Arc::new(Mem::new());
        mem.save_atomic(Path::new("/root/a.md"), b"hello").unwrap();
        let mut app = app_with_seed(&mem);
        let before = app.documents.len();

        open_path(&mut app, Path::new("/root/a.md"));

        assert_eq!(app.documents.len(), before + 1);
        assert_eq!(app.focus(), Pane::Editor);
        assert_eq!(
            app.active_doc().file_path.as_deref(),
            Some(Path::new("/root/a.md"))
        );
        assert!(!app.active_doc().is_store_bound());
    }

    #[test]
    fn opening_an_already_open_path_reactivates_instead_of_duplicating() {
        let mem = Arc::new(Mem::new());
        mem.save_atomic(Path::new("/root/a.md"), b"hello").unwrap();
        let mut app = app_with_seed(&mem);
        open_path(&mut app, Path::new("/root/a.md"));
        let after_first_open = app.documents.len();
        let first_active = app.active;

        open_path(&mut app, Path::new("/root/a.md"));

        assert_eq!(app.documents.len(), after_first_open, "must not duplicate");
        assert_eq!(app.active, first_active);
    }

    #[test]
    fn a_resolve_failing_path_posts_an_error_message_and_opens_nothing() {
        let mem = Arc::new(Mem::new());
        mem.fail_resolve(Path::new("/root/unresolvable.md"));
        let mut app = app_with_seed(&mem);
        let before = app.documents.len();

        let opened = open_path(&mut app, Path::new("/root/unresolvable.md"));

        assert!(opened.is_none());
        assert_eq!(app.documents.len(), before);
        assert!(
            messages::newest_text(&app).is_some(),
            "a resolve failure must post a message"
        );
    }

    #[test]
    fn a_resolve_failing_path_pushes_no_cmd_when_opened_async() {
        let mem = Arc::new(Mem::new());
        mem.fail_resolve(Path::new("/root/unresolvable.md"));
        let mut app = app_with_seed(&mem);
        let mut effects = Effects::default();

        open_path_async(
            &mut app,
            Path::new("/root/unresolvable.md"),
            None,
            &mut effects,
        );

        assert!(
            effects.cmds.is_empty(),
            "a resolve failure must never spawn a read Cmd"
        );
        assert!(
            messages::newest_text(&app).is_some(),
            "a resolve failure must post a message"
        );
    }

    #[test]
    fn a_missing_file_posts_an_error_message_and_opens_nothing() {
        let mem = Arc::new(Mem::new());
        let mut app = app_with_seed(&mem);
        let before = app.documents.len();

        open_path(&mut app, Path::new("/root/missing.md"));

        assert_eq!(app.documents.len(), before);
        assert!(
            messages::newest_text(&app).is_some(),
            "open failure must post a message"
        );
    }
}
