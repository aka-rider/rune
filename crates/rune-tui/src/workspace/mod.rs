use std::path::Path;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::codec;

use crate::app::App;
use crate::db_enqueue as db;
use crate::document::{DocumentId, ReadOnly};
use crate::focus::FocusChange;
use crate::graphics::{ImageState, ImageStatus};
use crate::help;
use crate::messages;
use crate::pane::Pane;
use crate::resolved::ResolvedPath;
use crate::runtime::{CmdError, Effects};

pub fn resolve_or_report(app: &mut App, path: &Path, verb: &str) -> Option<ResolvedPath> {
    match ResolvedPath::resolve(app.vfs.as_ref(), path) {
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
        ReadOutcome::Read { resolved, bytes } => Some(open_bytes(app, &resolved, bytes)),
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
            Some(open_bytes(app, &resolved, bytes))
        }
        ReadOutcome::Failed => None,
    }
}

enum ReadOutcome {
    Reactivated(DocumentId),
    Read {
        resolved: ResolvedPath,
        bytes: Vec<u8>,
    },
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

    match rune_vfs::get(app.vfs.as_ref(), &resolved, rune_vfs::MAX_DOCUMENT_BYTES) {
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
        if app.blur_title(effects) == FocusChange::Refused {
            return;
        }
        switch_to(app, id);
        app.set_focus_pane(Pane::Editor, effects);
        if let Some(anchor) = anchor {
            crate::navigate::land_anchor(app, id, &anchor);
        }
        return;
    }

    let vfs = Arc::clone(&app.vfs);
    effects.cmds.push(crate::runtime::read_file_cmd(
        vfs,
        resolved.into_path_buf(),
        anchor,
    ));
}

pub(crate) fn handle_file_opened(
    app: &mut App,
    path: &Path,
    result: Result<Vec<u8>, CmdError>,
    anchor: Option<rune_nav::Anchor>,
    preview_generation: Option<crate::generation::PreviewGen>,
    effects: &mut Effects,
) {
    if crate::explorer_preview::maybe_consume_reply(app, path, preview_generation, &result, effects)
    {
        crate::find::land::apply_pending_center(app);
        return;
    }

    // A kept title still gets the document: it is added as a background tab,
    // so the user's open request survives and the title keeps the keyboard.
    let title_kept = app.blur_title(effects) == FocusChange::Refused;

    let Some(resolved) = resolve_or_report(app, path, "open") else {
        return;
    };

    let id = if let Some(id) = existing_document_for(app, &resolved) {
        id
    } else {
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
        add_tab(app, &resolved, bytes)
    };
    if !title_kept {
        switch_to(app, id);
        app.set_focus_pane(Pane::Editor, effects);
    }
    if let Some(anchor) = anchor {
        crate::navigate::land_anchor(app, id, &anchor);
    }
}

pub fn warn_raw_bytes(app: &mut App, path: &Path, raw_bytes: usize) {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("this file");
    messages::warn(
        app,
        format!(
            "{name}: {raw_bytes} invalid UTF-8 byte(s) shown as <xx> \u{2014} written back unchanged"
        ),
    );
}

fn open_bytes(app: &mut App, resolved: &ResolvedPath, bytes: Vec<u8>) -> DocumentId {
    let id = add_tab(app, resolved, bytes);
    switch_to(app, id);
    id
}

// Adds the document as a tab and leaves the active document alone.
fn add_tab(app: &mut App, resolved: &ResolvedPath, bytes: Vec<u8>) -> DocumentId {
    if crate::document_support::is_image_path(resolved) {
        return add_image_tab(app, resolved, &bytes);
    }

    let buffer = Buffer::from_bytes(bytes);
    let raw_bytes = codec::count_raw_bytes(buffer.content());
    if raw_bytes > 0 {
        warn_raw_bytes(app, resolved, raw_bytes);
    }

    let id = app.open_document_bound(buffer, resolved.clone());
    let _ = db::load_document(app, id, resolved, db::LoadIntent::Recover);
    id
}

fn add_image_tab(app: &mut App, resolved: &ResolvedPath, bytes: &[u8]) -> DocumentId {
    let dims = rune_image::probe_dimensions(bytes).map(|(w, h, _)| rune_image::PixelSize { w, h });
    // Kitty image ids are terminal-global, so the whole-document image path
    // and the inline-embed path share one allocator keyed by this same
    // resolved-path string rather than a per-document hash, which could hand
    // two different open documents the same id.
    let key = resolved.to_string_lossy().into_owned();
    let id = app.image_ids.alloc_free_id(&key);
    let bytes_len = bytes.len() as u64;
    let file_name = resolved
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("image")
        .to_string();

    let doc_id = app.open_document_bound(Buffer::new(""), resolved.clone());
    if let Some(doc) = app.doc_mut(doc_id) {
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
    doc_id
}

pub(crate) fn existing_document_for(app: &App, path: &ResolvedPath) -> Option<DocumentId> {
    app.documents.document_for(path)
}

pub(crate) fn existing_document_for_spelling(app: &App, path: &Path) -> Option<DocumentId> {
    let resolved = ResolvedPath::resolve(app.vfs.as_ref(), path).ok()?;
    existing_document_for(app, &resolved)
}

pub(crate) fn shown_document_for(app: &App, path: &Path) -> Option<DocumentId> {
    let resolved = ResolvedPath::resolve(app.vfs.as_ref(), path).ok()?;
    if let Some(id) = existing_document_for(app, &resolved) {
        return Some(id);
    }
    let preview = app.explorer.preview.as_ref()?;
    (preview.doc.resolved_path() == Some(&resolved)).then_some(preview.id)
}

pub fn switch_to(app: &mut App, id: DocumentId) {
    if app.doc(id).is_none() {
        return;
    }
    crate::explorer_preview::discard(app);
    if app.merge.doc().is_some_and(|merge_doc| merge_doc != id) {
        crate::merge::auto_exit(app);
    }
    app.active = id;
    crate::find::accept_location(app);
    app.documents.touch(id);
    let name = crate::title::name_for(app.active_doc());
    app.title.seed(&name);
    if let Some(idx) = app.documents.order().iter().position(|&t| t == id) {
        app.tabs.nav.cursor = idx;
    }
    if !crate::db_enqueue::probe(app, id) {
        crate::messages::warn(
            app,
            "disk-sync check couldn't be scheduled for this file \u{2014} try switching tabs again",
        );
    }
}

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
    let sup_chords_reliable = crate::term::sup_chords_reliable(app.keyboard_flags);
    let id = app.open_document(Buffer::new(help::help_markdown(sup_chords_reliable)));
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
        assert_eq!(app.active_doc().path(), Some(Path::new("/root/a.md")));
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
    fn opening_the_absolute_spelling_of_a_relatively_opened_document_reactivates_it() {
        let mem = Arc::new(Mem::new());
        mem.save_atomic(Path::new("/note.md"), b"hello").unwrap();
        let mut app = app_with_seed(&mem);
        let id = open_path(&mut app, Path::new("note.md")).expect("the relative spelling opens");
        let before = app.documents.len();

        open_path(&mut app, Path::new("/note.md"));

        assert_eq!(app.documents.len(), before, "must not duplicate");
        assert_eq!(app.active, id);
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

    #[test]
    fn a_read_file_is_kept_as_a_background_tab_while_a_refused_title_holds_focus() {
        let mem = Arc::new(Mem::new());
        mem.save_atomic(Path::new("/root/new.md"), b"new body")
            .unwrap();
        let mut app = app_with_seed(&mem);
        let active_before = app.active;
        let tabs_before = app.documents.len();
        app.focus_title();
        app.title.set_text("bad/name");
        let posts_before = messages::posts(&app);

        let mut effects = Effects::default();
        handle_file_opened(
            &mut app,
            Path::new("/root/new.md"),
            Ok(b"new body".to_vec()),
            None,
            None,
            &mut effects,
        );

        assert_eq!(
            app.documents.len(),
            tabs_before + 1,
            "the read file is kept as a tab"
        );
        assert_eq!(
            app.active, active_before,
            "a kept title must not switch the document"
        );
        assert_eq!(
            app.focus(),
            Pane::Title,
            "the invalid name keeps the keyboard"
        );
        assert_eq!(
            messages::posts(&app),
            posts_before + 1,
            "the refusal posts once"
        );
    }
}
