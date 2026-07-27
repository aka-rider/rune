//! `workspace::open_path` — opens a file selected in the Explorer (plan
//! WP4.S5): re-activates a `Document` already bound to the resolved path,
//! or reads a fresh one SYNCHRONOUSLY through the injected `Vfs` (§1.4.9).
//! Directory navigation is the Explorer's own job instead
//! (`explorer::handle_key`'s `Open`/`ParentDir` arms build a `runtime::
//! load_dir_cmd` `Cmd` directly) — this module only ever opens FILES, so
//! unlike that path it needs no `Cmd`/`Effects` (a single `vfs.read` is
//! cheap enough to run inline here, exactly as the pre-WP4 bootstrap load
//! in `rune-cli::main::load_buffer` already does).

use std::path::Path;

use rune_core::buffer::{Buffer, BufferError};

use crate::app::{App, StatusSource};
use crate::document::DocumentId;
use crate::pane::Pane;

/// Opens `path`: normalizes it via `app.vfs.resolve`, then either
/// re-activates an already-open `Document` with that resolved `file_path`
/// or reads a fresh one. A read/decode failure is reported through
/// `app.set_status` — WP3's `report_error` (the error Banner) is not yet on
/// this base.
// WP3: route through report_error once the banner modal exists on this base.
pub fn open_path(app: &mut App, path: &Path) {
    let resolved = app.vfs.resolve(path).unwrap_or_else(|_| path.to_path_buf());

    if let Some(id) = existing_document_for(app, &resolved) {
        app.active = id;
        app.focus = Pane::Editor;
        return;
    }

    let bytes = match app.vfs.read(&resolved) {
        Ok(bytes) => bytes,
        Err(e) => {
            app.set_status(
                format!("could not open {}: {e}", resolved.display()),
                StatusSource::Other,
            );
            return;
        }
    };

    let buffer = match Buffer::from_bytes(bytes) {
        Ok(buffer) => buffer,
        Err(BufferError::InvalidUtf8) => {
            app.set_status(
                format!(
                    "{}: not valid UTF-8 \u{2014} refusing to open",
                    resolved.display()
                ),
                StatusSource::Other,
            );
            return;
        }
        Err(e) => {
            app.set_status(
                format!("could not open {}: {e}", resolved.display()),
                StatusSource::Other,
            );
            return;
        }
    };

    let id = app.open_document(buffer);
    if let Some(doc) = app.doc_mut(id) {
        doc.file_path = Some(resolved);
    }
    app.active = id;
    app.focus = Pane::Editor;
    // Assumption A1 (plan): an Explorer-opened document has no per-doc
    // recovery journal yet — `Document::db` stays `None` from `App::
    // open_document`. See TODO.md, "per-doc recovery hydration for
    // explorer-opened documents" (dated 2026-07-27).
    app.set_status("no crash recovery for this tab yet", StatusSource::Other);
}

fn existing_document_for(app: &App, path: &Path) -> Option<DocumentId> {
    app.documents
        .iter()
        .find(|(_, doc)| doc.file_path.as_deref() == Some(path))
        .map(|(id, _)| *id)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use rune_core::buffer::Buffer as CoreBuffer;
    use rune_vfs::{Mem, Vfs};
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
        assert_eq!(app.focus, Pane::Editor);
        assert_eq!(
            app.active_doc().file_path.as_deref(),
            Some(Path::new("/root/a.md"))
        );
        assert!(app.active_doc().db.is_none());
    }

    #[test]
    fn opening_an_already_open_path_reactivates_instead_of_duplicating() {
        let mem = Arc::new(Mem::new());
        mem.save_atomic(Path::new("/root/a.md"), b"hello").unwrap();
        let mut app = app_with_seed(&mem);
        open_path(&mut app, Path::new("/root/a.md"));
        let after_first_open = app.documents.len();
        let first_active = app.active;

        // Switch focus elsewhere so re-activation is actually observable.
        app.focus = Pane::Explorer;
        open_path(&mut app, Path::new("/root/a.md"));

        assert_eq!(app.documents.len(), after_first_open, "must not duplicate");
        assert_eq!(app.active, first_active);
        assert_eq!(app.focus, Pane::Editor);
    }

    #[test]
    fn a_missing_file_reports_a_status_error_and_opens_nothing() {
        let mem = Arc::new(Mem::new());
        let mut app = app_with_seed(&mem);
        let before = app.documents.len();

        open_path(&mut app, Path::new("/root/missing.md"));

        assert_eq!(app.documents.len(), before);
        assert!(app.status_message.is_some());
    }
}
