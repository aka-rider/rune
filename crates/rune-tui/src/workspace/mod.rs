//! `workspace::open_path` — opens a file selected in the Explorer (plan
//! WP4.S5): re-activates a `Document` already bound to the resolved path,
//! or reads a fresh one SYNCHRONOUSLY through the injected `Vfs` (§1.4.9).
//! Directory navigation is the Explorer's own job instead
//! (`explorer_keys::handle_key`'s `Open`/`ParentDir` arms build a `runtime::
//! load_dir_cmd` `Cmd` directly) — this module only ever opens FILES, so
//! unlike that path it needs no `Cmd`/`Effects` (a single `vfs.read` is
//! cheap enough to run inline here, exactly as the pre-WP4 bootstrap load
//! in `rune-cli::main::load_buffer` already does).

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
use crate::runtime::Effects;

/// Normalizes `path` through the injected `Vfs` — the ONE resolution
/// chokepoint every path that will ever bind a `Document` funnels through
/// (`open_path` below, and `rune-cli::main`'s bootstrap open of the first
/// CLI positional), so the same underlying file can never bind as two
/// textually different documents (a symlink, a `..` segment, or a
/// duplicated absolute-path spelling all collapse to one canonical form
/// here instead of staying an unresolved, divergence-prone primitive).
/// Falls back to `path` itself only when resolution itself fails (e.g.
/// permission denied) — the caller's own subsequent read surfaces that
/// same failure.
pub fn resolve(vfs: &dyn Vfs, path: &Path) -> PathBuf {
    vfs.resolve(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Opens `path`: normalizes it via [`resolve`], then either re-activates
/// an already-open `Document` with that resolved `file_path` or reads a
/// fresh one. A read/decode failure is reported into the message log
/// (`messages::error`) — the one chokepoint every error report
/// funnels through. Returns the opened/reactivated
/// document's id, or `None` on any of the error paths — `navigate::follow`
/// (plan WP5) needs the id back to force-parse the target and land the
/// caret on an anchor.
pub fn open_path(app: &mut App, path: &Path) -> Option<DocumentId> {
    let resolved = resolve(app.vfs.as_ref(), path);

    if let Some(id) = existing_document_for(app, &resolved) {
        // Re-activation moves the Tabs cursor only — never reorders
        // `tabs.order` (plan WP5.S1's own chokepoint list).
        switch_to(app, id);
        return Some(id);
    }

    let bytes = match app.vfs.read(&resolved) {
        Ok(bytes) => bytes,
        Err(e) => {
            messages::error(app, format!("could not open {}: {e}", resolved.display()));
            return None;
        }
    };
    open_bytes(app, &resolved, bytes)
}

/// Opens `path` off-thread (plan WP5.S6, [rune-tui A 7]: "synchronous
/// `vfs.read` inside `update` blocks the Elm loop") — the interactive-
/// navigation counterpart of [`open_path`] above. An already-open document
/// still reactivates synchronously (no I/O to wait on); a fresh path spawns
/// a `ReadFile` `Cmd` and returns immediately with nothing landed yet — the
/// eventual `Msg::FileOpened` ack ([`handle_file_opened`], routed via
/// `dispatch::update_inner`) finishes opening the document and lands
/// `anchor`, if given, through `navigate::land_anchor`.
///
/// The pre-runtime CLI bootstrap (`rune-cli::main`'s multi-file launch —
/// no `Effects` sink or `Msg` loop exists yet to reply into) and the
/// Explorer's own Open-on-a-file path still call the synchronous
/// [`open_path`] directly; this is `navigate::follow`'s entry point only.
pub fn open_path_async(
    app: &mut App,
    path: &Path,
    anchor: Option<rune_nav::Anchor>,
    effects: &mut Effects,
) {
    let resolved = resolve(app.vfs.as_ref(), path);

    if let Some(id) = existing_document_for(app, &resolved) {
        // Decision 8: blur BEFORE the switch — `switch_to` is about to
        // reassign `app.active`, and `rename::begin` resolves its subject
        // from the live `app.active`, so blurring after would rename the
        // document just reactivated, not the one the user was renaming.
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

/// The reaction to a `ReadFile` `Cmd`'s completion (`Msg::FileOpened`) — the
/// async counterpart of `open_path`'s own inline read-then-insert tail.
/// Rechecks `existing_document_for` before inserting: a second navigation to
/// the same path issued while this read was in flight may have already
/// opened it by some other route, and inserting again would duplicate the
/// document rather than reactivating it.
///
/// Blurs the title FIRST, unconditionally (decision 8) — this is an async
/// ack, so the title can genuinely be focused when it lands, and the switch
/// below (whichever branch reaches it) must never fire against the wrong
/// document. Only lands focus on the Editor on the paths that actually open
/// something; a read/decode failure posts an error message instead and must
/// not steal the keyboard from wherever it already was.
pub(crate) fn handle_file_opened(
    app: &mut App,
    path: PathBuf,
    result: Result<Vec<u8>, String>,
    anchor: Option<rune_nav::Anchor>,
    effects: &mut Effects,
) {
    // The Explorer's live preview reads a file through this SAME `Msg::
    // FileOpened` channel (`runtime::read_preview_cmd`) rather than a
    // second one — `explorer_preview::maybe_consume_reply` claims a reply
    // only when it was the one that asked for it (`Explorer::
    // preview_awaiting`), so an ordinary open below never sees one of its
    // own requests intercepted.
    if crate::explorer_preview::maybe_consume_reply(app, &path, &result) {
        return;
    }

    app.blur_title(effects);

    if let Some(id) = existing_document_for(app, &path) {
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
    let Some(id) = open_bytes(app, &path, bytes) else {
        return;
    };
    app.set_focus_pane(Pane::Editor, effects);
    if let Some(anchor) = anchor {
        crate::navigate::land_anchor(app, id, &anchor);
    }
}

/// The decode-then-insert tail shared by [`open_path`]'s inline read and
/// [`handle_file_opened`]'s async one: decode `bytes` as a `Buffer`, insert
/// a new `Document` bound to `resolved`, hydrate it through the recovery
/// store, and switch to it. Reports and returns `None` on a decode failure
/// — the caller's own read already succeeded by this point.
///
/// Branches BEFORE `Buffer::from_bytes` for an image path (plan WP4.S7):
/// image bytes are never valid UTF-8 in general (`Buffer` is UTF-8 by
/// type), and even a coincidentally UTF-8-clean image must still open as a
/// read-only image document, not an editable text one. Covers both the
/// Explorer's open (`open_path`) and the CLI's extra positionals
/// (`open_path` again, via `rune-cli`'s `open::open_extra_files`) — both
/// funnel through this one tail.
fn open_bytes(app: &mut App, resolved: &Path, bytes: Vec<u8>) -> Option<DocumentId> {
    if crate::document_support::is_image_path(resolved) {
        return open_image_bytes(app, resolved, bytes);
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
    // Hydrates `id` through the app-wide recovery store (plan WP6, closing
    // the gap TODO.md's "per-doc recovery hydration for explorer-opened
    // documents" entry records): non-blocking, ack-driven via
    // `app::handle_db_event`'s `Load` arm — `Document::db` stays `None`
    // until that ack lands (or forever, if this store is absent/degraded).
    db::load_document(app, id, resolved);
    switch_to(app, id);
    Some(id)
}

/// The image-document counterpart of `open_bytes`'s ordinary tail (plan
/// WP4.S7): an always-empty `Buffer` (image bytes never live in one), read-
/// only, no recovery binding at all — there is no text journal to hydrate
/// and `save::trigger_save`'s own `DocumentKind::Image` guard means nothing
/// would ever write through it anyway. `bind_path` still runs first (it is
/// the one place `kind` is derived, and `kind_for` above already agrees the
/// result will be `Image`), so `doc.kind` and `doc.doc`'s producer selection
/// come from the SAME chokepoint every other document uses — only the
/// `display_name`/`read_only`/`image` fields it doesn't itself set are
/// layered on afterward. `probe_dimensions` is header-only (no full decode,
/// no `ratatui`/protocol dependency) — enough for the info card to show
/// `WIDTHxHEIGHT` before any decode `Cmd` exists at all (WP5).
fn open_image_bytes(app: &mut App, resolved: &Path, bytes: Vec<u8>) -> Option<DocumentId> {
    let dims = rune_image::probe_dimensions(&bytes).map(|(w, h, _)| (w, h));
    let id = rune_image::alloc_id(&resolved.to_string_lossy());
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
        doc.image = Some(ImageState {
            path: resolved.to_path_buf(),
            bytes_len,
            id,
            dims,
            cells: None,
            decoded: None,
            status: ImageStatus::Pending,
            in_flight: None,
            next_generation: 0,
        });
    }
    switch_to(app, doc_id);
    Some(doc_id)
}

pub(crate) fn existing_document_for(app: &App, path: &Path) -> Option<DocumentId> {
    app.documents
        .iter()
        .find(|(_, doc)| doc.file_path.as_deref() == Some(path))
        .map(|(id, _)| *id)
}

/// Switches the active document to `id` — the chokepoint plan WP5.S2 asks
/// for (Open Tabs' `Select`), reused by `open_path`'s re-activation path
/// above. A no-op if `id` doesn't reference a live document (a stale id from
/// some racing close). Also moves — never reorders — the Tabs pane's own
/// cursor to `id`'s position in `tabs.order`, so the next Up/Down there
/// starts from the tab that's actually now showing.
///
/// Writes no focus of its own (plan decision 6): this is the one function
/// that could blur the title without an `Effects` sink, so every caller now
/// does that itself, in prefix position, BEFORE calling this — `rename::
/// begin` resolves its subject from the live `app.active`, so blurring
/// AFTER this assignment would rename the newly-opened document, not the
/// outgoing one. Every caller then decides separately (and conditionally,
/// wherever the switch itself can fail) where focus should land afterwards.
pub fn switch_to(app: &mut App, id: DocumentId) {
    if app.doc(id).is_none() {
        return;
    }
    // Switching the active document AWAY from the live Explorer preview is
    // exactly the "user selected away" moment the preview must not survive
    // (`^1`-`^0`, `TabsCommand::Select`, re-activating an already-open
    // document from the Explorer, ... every one of them funnels through
    // this chokepoint). A switch back ONTO the preview itself (`id` already
    // matches) leaves it untouched — that's promotion's job, not this
    // one's. No neighbor reassignment is needed here even when `app.active`
    // currently IS the discarded preview: the very next line below reseats
    // it at `id`.
    crate::explorer_preview::discard_if_switching_away(app, id);
    // Plan WP6.S3, decision 12: switching AWAY from the merge document is an
    // implicit Esc — auto-exit BEFORE the title reseed below, or `name_for`
    // would re-derive the OUTGOING document's plain name while `app.merge`
    // still holds it `Active`, leaving the merge suffix to vanish from a
    // title that then immediately gets overwritten anyway. A same-document
    // "switch" (`id` already active) is not a transition at all. `auto_exit`
    // (review fix F3) also cancels a `Pending` attempt WITH feedback,
    // rather than `exit_in_place` silently discarding it (`Pending` has no
    // working form to exit from at all).
    if app.merge.doc().is_some_and(|merge_doc| merge_doc != id) {
        crate::merge::auto_exit(app);
    }
    app.active = id;
    // The title field describes the ACTIVE document, so it is reseeded at
    // the one chokepoint every switch funnels through — never left holding
    // the previous document's name (no shadow state).
    let name = crate::title::name_for(app.active_doc());
    app.title.seed(&name);
    if let Some(idx) = app.tabs.order.iter().position(|&t| t == id) {
        app.tabs.nav.cursor = idx;
    }
    // Plan WP2.S4: re-check this document's disk fact on every switch onto
    // it — the only detection wiring besides `Load`-at-open and the
    // save-time CAS conflict (plan decision 8: no file watcher).
    crate::db_enqueue::probe(app, id);
}

/// Switches to the tab sitting at `idx` in the current tab order, if there
/// is one. Positional rather than by id, for the callers that only know a
/// row or a typed digit; an index past the end is a silent no-op, so a
/// digit chord naming a tab that isn't open simply does nothing. Funnels
/// into [`switch_to`], so a positional switch can never skip the title
/// reseed or the cursor move.
pub fn switch_to_index(app: &mut App, idx: usize) {
    if let Some(&id) = app.tabs.order.get(idx) {
        switch_to(app, id);
    }
}

/// `F1` (plan WP7.S2, `keymap::GlobalCommand::Help`): mints the read-only
/// Help virtual document the first time it's ever needed (`App.help_doc`,
/// idempotent — a second press never mints a duplicate), then toggles
/// between it and whatever was active before. Unlike Go's `toggleHelp`
/// (`workspace_nav.go`, which CLOSES the help tab on the second press
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
        doc.read_only = ReadOnly::Always;
        doc.display_name = Some("Help".to_string());
    }
    app.help_doc = Some(id);
    app.help_return_to = Some(previous);
    switch_to(app, id);
}

// `request_close`/`close_now`/`neighbor_of` moved to `workspace::close`
// (§1.6 budget, WP5.S7's image-delete-on-close hook) — re-exported below so
// every existing `workspace::` call site keeps working unchanged.
pub(crate) mod close;
pub use close::{close_now, new_untitled_document, next_untitled_name, request_close};

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
        assert_eq!(app.focus(), Pane::Editor);
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

        // `open_path` itself no longer moves focus (plan WP2 decision 6:
        // `switch_to` lost that write, and this function has no `Effects`
        // sink to run `App::set_focus` through) — this test's own re-
        // activation contract is `documents.len()`/`active` staying put, not
        // a focus assertion this change removes (plan gotcha 7).
        open_path(&mut app, Path::new("/root/a.md"));

        assert_eq!(app.documents.len(), after_first_open, "must not duplicate");
        assert_eq!(app.active, first_active);
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
