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
use crate::banner::{self, GuardKind, GuardPrompt, Modal};
use crate::document::DocumentId;
use crate::help;
use crate::pane::Pane;

/// Opens `path`: normalizes it via `app.vfs.resolve`, then either
/// re-activates an already-open `Document` with that resolved `file_path`
/// or reads a fresh one. A read/decode failure is reported through the
/// error Banner (`banner::report_error`) — the one chokepoint every error
/// report funnels through (plan WP3.S4).
pub fn open_path(app: &mut App, path: &Path) {
    let resolved = app.vfs.resolve(path).unwrap_or_else(|_| path.to_path_buf());

    if let Some(id) = existing_document_for(app, &resolved) {
        // Re-activation moves the Tabs cursor only — never reorders
        // `tabs.order` (plan WP5.S1's own chokepoint list).
        switch_to(app, id);
        return;
    }

    let bytes = match app.vfs.read(&resolved) {
        Ok(bytes) => bytes,
        Err(e) => {
            crate::banner::report_error(app, format!("could not open {}: {e}", resolved.display()));
            return;
        }
    };

    let buffer = match Buffer::from_bytes(bytes) {
        Ok(buffer) => buffer,
        Err(BufferError::InvalidUtf8) => {
            crate::banner::report_error(
                app,
                format!(
                    "{}: not valid UTF-8 \u{2014} refusing to open",
                    resolved.display()
                ),
            );
            return;
        }
        Err(e) => {
            crate::banner::report_error(app, format!("could not open {}: {e}", resolved.display()));
            return;
        }
    };

    let id = app.open_document(buffer);
    if let Some(doc) = app.doc_mut(id) {
        doc.file_path = Some(resolved);
    }
    switch_to(app, id);
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

/// Switches the active document to `id` and focuses the editor — the
/// chokepoint plan WP5.S2 asks for (Open Tabs' `Select`), reused by
/// `open_path`'s re-activation path above. A no-op if `id` doesn't
/// reference a live document (a stale id from some racing close). Also
/// moves — never reorders — the Tabs pane's own cursor to `id`'s position
/// in `tabs.order`, so the next Up/Down there starts from the tab that's
/// actually now showing.
pub fn switch_to(app: &mut App, id: DocumentId) {
    if app.doc(id).is_none() {
        return;
    }
    app.active = id;
    app.focus = Pane::Editor;
    // The title field describes the ACTIVE document, so it is reseeded at
    // the one chokepoint every switch funnels through — never left holding
    // the previous document's name (no shadow state).
    let stem = crate::title::stem_for(app.active_doc());
    app.title.seed(&stem);
    if let Some(idx) = app.tabs.order.iter().position(|&t| t == id) {
        app.tabs.nav.cursor = idx;
    }
}

/// `F1` (plan WP7.S2, `keymap::GlobalCommand::Help`): mints the read-only
/// Help virtual document the first time it's ever needed (`App.help_doc`,
/// idempotent — a second press never mints a duplicate), then toggles
/// between it and whatever was active before. Unlike Go's `toggleHelp`
/// (`workspace_nav.go:152`, which CLOSES the help tab on the second press
/// while focused there), this port keeps the Help document as an ordinary,
/// closable tab and instead switches back to `App.help_return_to` — the
/// document that was active right before Help was last activated. Falls
/// back to any other live document if that one has since been closed
/// (e.g. via the Tabs pane's own `^w`). If the Help document ITSELF has
/// since been closed the same way, `live_help`'s `documents.contains_key`
/// check below fails and this mints a fresh one — `App.help_doc` never
/// points at a stale id.
pub fn toggle_help(app: &mut App) {
    let live_help = app.help_doc.filter(|id| app.documents.contains_key(id));

    if let Some(id) = live_help {
        if app.active == id {
            let target = app
                .help_return_to
                .filter(|t| *t != id && app.documents.contains_key(t))
                .or_else(|| app.documents.keys().find(|&&other| other != id).copied())
                .unwrap_or(id);
            switch_to(app, target);
        } else {
            app.help_return_to = Some(app.active);
            switch_to(app, id);
        }
        return;
    }

    let previous = app.active;
    let id = app.open_document(Buffer::new(help::help_markdown()));
    if let Some(doc) = app.doc_mut(id) {
        doc.read_only = true;
        doc.display_name = Some("Help".to_string());
    }
    app.help_doc = Some(id);
    app.help_return_to = Some(previous);
    switch_to(app, id);
}

/// Requests closing `id` (plan WP5.S3): refuses outright if it's the LAST
/// remaining document (rune always shows one — the WP1 accessor floor on
/// `App::active_doc`/`active_doc_mut` depends on `documents` staying
/// non-empty), closes immediately if `id` is clean, or arms the close-guard
/// modal if it's dirty. A stale/already-closed `id` is a silent no-op.
pub fn request_close(app: &mut App, id: DocumentId) {
    if app.documents.len() <= 1 {
        app.set_status("can't close the last open document", StatusSource::Other);
        return;
    }
    let Some(doc) = app.doc(id) else { return };
    if doc.is_dirty() {
        // An `Error` already up outranks this prompt; the close intent is
        // then simply not armed (the user presses `^w` again once the
        // error is dismissed) — nothing waits on this Guard, unlike the
        // rename machine's.
        let _ = banner::set_modal(
            app,
            Modal::Guard(GuardPrompt {
                doc: id,
                kind: GuardKind::DirtyClose,
            }),
        );
    } else {
        close_now(app, id);
    }
}

/// Closes `id` unconditionally — the plan WP5.S3 chokepoint every close
/// path (clean `request_close`, the Guard's `[D]iscard`, and its `[S]ave`
/// once the save ack lands) funnels through. Reassigns `active` to a
/// neighbor FIRST when `id` is the active document — per the WP1 invariant
/// comment on `App::active_doc`/`active_doc_mut`, `active` must always
/// reference a live entry, so the reassignment happens before `id` is
/// removed, never after. Refuses to remove the LAST document (the same
/// floor `request_close` already enforces, kept here too since `close_now`
/// is reachable from other callers that don't re-check it themselves).
/// Sweeps `db_ops` of any entry still pointing at `id` — a stale ack would
/// already be a correct no-op via `App::doc_mut` returning `None` (see its
/// docs), but leaving the entry forever would make `db_ops` an unbounded
/// leak over a long session of open/close cycles. Clears `pending_close_on_
/// save`/`pending_save_confirm` when either still targets `id` (review fix
/// for the latter — it was left dangling): both are doc-tagged `Option`s
/// that would otherwise point at a document that no longer exists, e.g. a
/// stray `SaveConfirmTimeout` generation match resurrecting a confirm gate
/// for a doc `[D]iscard` just closed.
pub fn close_now(app: &mut App, id: DocumentId) {
    if app.documents.len() <= 1 || !app.documents.contains_key(&id) {
        return;
    }
    if app.active == id
        && let Some(neighbor) = neighbor_of(app, id)
    {
        app.active = neighbor;
    }
    app.documents.remove(&id);
    app.tabs.order.retain(|&t| t != id);
    app.db_ops.retain(|_, doc_id| *doc_id != id);
    if app.pending_close_on_save == Some(id) {
        app.pending_close_on_save = None;
    }
    if app.pending_save_confirm.is_some_and(|(cid, _)| cid == id) {
        app.pending_save_confirm = None;
    }
    // The rename machine is one more doc-tagged pending slot to sweep
    // (plan's transition table: "any | close_now(doc) | Idle").
    crate::rename::forget_document(app, id);

    app.tabs.nav.cursor = app
        .tabs
        .order
        .iter()
        .position(|&t| t == app.active)
        .unwrap_or(0);
}

/// The neighbor `close_now` reassigns `active` to when the closed document
/// WAS active: the next tab in `tabs.order`, else the previous one (plan
/// WP5.S3). Falls back to any other live document if `id` isn't in
/// `tabs.order` at all — shouldn't happen (every document has a tab), but
/// keeps this total rather than leaving `active` dangling.
fn neighbor_of(app: &App, id: DocumentId) -> Option<DocumentId> {
    if let Some(idx) = app.tabs.order.iter().position(|&t| t == id) {
        if let Some(&next) = app.tabs.order.get(idx + 1) {
            return Some(next);
        }
        if idx > 0 {
            return app.tabs.order.get(idx - 1).copied();
        }
    }
    app.documents.keys().find(|&&k| k != id).copied()
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
    fn a_missing_file_raises_the_error_banner_and_opens_nothing() {
        let mem = Arc::new(Mem::new());
        let mut app = app_with_seed(&mem);
        let before = app.documents.len();

        open_path(&mut app, Path::new("/root/missing.md"));

        assert_eq!(app.documents.len(), before);
        assert!(app.modal.is_some(), "open failure must raise the Banner");
    }
}
