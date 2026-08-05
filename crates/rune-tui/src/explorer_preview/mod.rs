//! Explorer live-preview: moving the Explorer cursor shows the file under
//! it in the Editor immediately, without minting a permanent tab the way
//! `workspace::open_path` does. The previewed document occupies a REAL slot
//! in `tabs.order` (`ReadOnly::Preview`, `document/mod.rs`) rather than an
//! out-of-band one — that's deliberate, so `^1`-`^0`, the Tabs pane, and
//! `workspace::close`'s own neighbour logic all keep working on it by
//! construction instead of needing an untabbed-active special case.
//!
//! Three entry points drive the whole lifecycle:
//! - [`after_cursor_move`] — called after every Explorer cursor move
//!   (`explorer_keys::move_selection`/Top/Bottom, and once when a live
//!   search clears) — resolves what the cursor now sits on and either
//!   reactivates an already-open document, asks the `Vfs` to read a new
//!   one, or does nothing (a directory, or a read already in flight).
//! - [`maybe_consume_reply`] — the async reply's landing point, called from
//!   `workspace::handle_file_opened` before its own ordinary-open logic
//!   ever runs.
//! - [`on_focus_changed`] — called from `app::update`'s post-dispatch
//!   chokepoint with the focus pane observed before and after the message:
//!   focus landing on the Editor promotes the live preview (if any);
//!   focus landing on the Title or Tabs pane discards it. Neither of those
//!   transitions is reachable through this crate's own `switch_to`
//!   chokepoint (a pure focus move touches no document), so this is its own
//!   entry point rather than folded into [`discard_if_switching_away`].
//!
//! [`discard_if_switching_away`] is the fourth, `workspace::switch_to`'s own
//! hook: every switch to a DIFFERENT active document — `^1`-`^0`,
//! `TabsCommand::Select`, re-activating an already-open document from the
//! Explorer — discards a stale preview by construction, with no call site
//! outside this module needing to remember to do it.

use std::path::Path;

use rune_core::buffer::Buffer;

use crate::app::App;
use crate::document::{Document, DocumentId, ReadOnly};
use crate::pane::Pane;
use crate::runtime::Effects;
use crate::workspace;

/// Resolves what the Explorer cursor sits on right now and reacts:
/// silently does nothing for a directory (the synthetic `..` row included)
/// or while a type-to-search query is live (design: "no preview while
/// type-to-search is live" — a search's own cursor jumps call
/// `apply_search` directly, never this function, but the guard stays here
/// too since `explorer_search::handle_search`'s clear points call this
/// unconditionally). A file already open as a real tab is shown via its
/// own live document — no second read, no duplicate, and any STALE minted
/// preview from a moment ago is discarded as part of the ordinary
/// `workspace::switch_to` a reactivation already runs through. A file with
/// no document yet asks the `Vfs` to read it, unless a read for that exact
/// path is already in flight.
pub(crate) fn after_cursor_move(app: &mut App, effects: &mut Effects) {
    if app.explorer.search.is_some() {
        return;
    }
    let Some(entry) = app.explorer.entries.get(app.explorer.nav.cursor) else {
        return;
    };
    if entry.is_dir {
        return;
    }
    let target = entry.path.clone();

    if let Some(id) = workspace::existing_document_for(app, &target) {
        workspace::switch_to(app, id);
        return;
    }

    let already_showing = app
        .explorer
        .preview
        .and_then(|id| app.doc(id))
        .and_then(|doc| doc.file_path.as_deref())
        == Some(target.as_path());
    if already_showing || app.explorer.preview_awaiting.contains(&target) {
        return;
    }

    app.explorer.preview_awaiting.insert(target.clone());
    let vfs = std::sync::Arc::clone(&app.vfs);
    effects
        .cmds
        .push(crate::runtime::read_preview_cmd(vfs, target));
}

/// Claims a `Msg::FileOpened` reply this module itself requested, returning
/// whether it did — `workspace::handle_file_opened` skips its own ordinary
/// open logic entirely when this returns `true`, so a preview read can
/// never ALSO mint a second, permanent tab. A reply this module never asked
/// for (`preview_awaiting` doesn't contain `path`) is left untouched,
/// exactly as before this module existed.
///
/// Two more checks run before the bytes are ever adopted, both of which
/// drop the reply silently rather than showing it: `path` already has a
/// real (non-preview) document — the ordinary open-file/follow-link path
/// beat this read to it while it was in flight — or the Explorer cursor has
/// since moved off `path` entirely, so the reply is stale relative to
/// whatever the cursor is showing now. Neither ever falls through to the
/// ordinary open logic: once this module owns a reply, it owns every
/// outcome for it, including doing nothing.
pub(crate) fn maybe_consume_reply(
    app: &mut App,
    path: &Path,
    result: &Result<Vec<u8>, String>,
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
    if let Ok(bytes) = result {
        apply_loaded(app, path, bytes.clone());
    }
    true
}

/// Whether `path` is still what the Explorer cursor is sitting on — the
/// staleness check `maybe_consume_reply` applies to every reply it claims.
/// Re-derived fresh from the live cursor position rather than a stored
/// generation counter: `preview_awaiting` already guarantees at most one
/// live request per path, so the only question worth asking on arrival is
/// "does the cursor still agree", not "which request number was this".
fn is_current_target(app: &App, path: &Path) -> bool {
    if app.explorer.search.is_some() {
        return false;
    }
    app.explorer
        .entries
        .get(app.explorer.nav.cursor)
        .is_some_and(|e| !e.is_dir && e.path == path)
}

/// Mints the FIRST preview of this Explorer session, or replaces the
/// content and path of the ONE already minted — never a second tab. Sets
/// `app.explorer.preview` before calling `workspace::switch_to` so that
/// chokepoint's own discard check (`discard_if_switching_away`) sees the
/// slot already pointing at the document it's about to switch to and
/// leaves it alone.
///
/// The reused-id branch advances the freshly loaded buffer's version past
/// the document it's about to replace (`Buffer::advance_past`) before
/// `Document::new` ever sees it. A preview's buffer is never edited, so
/// without this every preview after the first would sit at version 1
/// forever under the same id — indistinguishable, to a version-gated
/// consumer, from "nothing changed". Two such consumers key off exactly
/// that: `dispatch::after_update`'s highlight-reschedule check (a preview
/// with no colours until the version moves) and `dispatch::
/// handle_highlighted`'s in-flight-reply staleness guard (a reply for the
/// file just replaced, arriving after the swap, whose stale version could
/// otherwise coincide with the new buffer's and get installed onto it).
/// Monotonic versions make that coincidence unrepresentable rather than
/// merely unlikely.
fn apply_loaded(app: &mut App, path: &Path, bytes: Vec<u8>) {
    let Ok(buffer) = Buffer::from_bytes(bytes) else {
        // `read_preview_cmd` already rejected invalid UTF-8 before this
        // reply was ever sent — unreachable in practice, kept as a refusal
        // rather than a silent corruption of whatever is on screen.
        return;
    };
    let id = match app.explorer.preview.filter(|id| app.doc(*id).is_some()) {
        Some(id) => {
            let floor = app.doc(id).map_or(0, |doc| doc.buffer.version());
            let buffer = buffer.advance_past(floor);
            if let Some(doc) = app.doc_mut(id) {
                *doc = Document::new(buffer);
                doc.bind_path(path.to_path_buf());
                doc.read_only = ReadOnly::Preview;
            }
            id
        }
        None => {
            let id = app.open_document(buffer);
            if let Some(doc) = app.doc_mut(id) {
                doc.bind_path(path.to_path_buf());
                doc.read_only = ReadOnly::Preview;
            }
            app.explorer.preview = Some(id);
            // Remembers what the user was editing before this browsing
            // session's preview took over — `discard_active`'s own way
            // back, captured now because `switch_to` below is about to
            // move `app.active` off it.
            app.explorer.preview_return_to = Some(app.active);
            id
        }
    };
    workspace::switch_to(app, id);
}

/// `workspace::switch_to`'s own hook (called at the top of every switch,
/// before `app.active` moves): discards the live preview whenever the
/// switch's target is a DIFFERENT document. A switch back onto the preview
/// itself (`target == id`, e.g. `apply_loaded`'s own switch, or the
/// Explorer cursor landing back on the file it already shows) leaves it
/// untouched. No neighbour reassignment runs here even when `app.active`
/// currently names the discarded preview — `switch_to`'s very next line
/// reseats `app.active` at `target`.
pub(crate) fn discard_if_switching_away(app: &mut App, target: DocumentId) {
    let Some(id) = app.explorer.preview else {
        return;
    };
    if id == target {
        return;
    }
    remove_preview_document(app, id);
}

/// Reacts to a focus transition observed around one `update` call
/// (`app::update`, which has no other hook for a pure focus move — neither
/// `workspace::switch_to` nor any Explorer key handler runs when focus
/// alone changes). `previous == current` is the overwhelming majority of
/// messages and costs one comparison. The Editor is the only promotion
/// target; the Title and Tabs panes are the two panes the design calls out
/// as immediate discards. Landing back on the Explorer itself, or a focus
/// move this module has no opinion on, does nothing.
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
        // Landing on the Explorer itself, or on the messages pane (plan
        // WP1 — a focus move this module has no opinion on, exactly like
        // the Explorer case), does nothing.
        Pane::Explorer | Pane::Messages => {}
    }
}

/// Promotes `id` from a transient preview into the document the user is
/// actually editing: drops the `Preview` read-only gate and, only now,
/// hydrates it through the recovery store — a preview never contacts the
/// store before this point (design: "no recovery-store contact until
/// promotion"). A no-op unless `id` is still the live preview slot, so a
/// stale call (the focus-changed hook firing after Enter already promoted
/// this same document moments earlier) never re-hydrates it a second time.
pub(crate) fn promote(app: &mut App, id: DocumentId) {
    if app.explorer.preview != Some(id) {
        return;
    }
    if let Some(doc) = app.doc_mut(id) {
        doc.read_only = ReadOnly::No;
    }
    app.explorer.preview = None;
    app.explorer.preview_return_to = None;
    let path = app.doc(id).and_then(|doc| doc.file_path.clone());
    if let Some(path) = path {
        crate::db_enqueue::load_document(app, id, &path);
    }
}

/// Discards the live preview because focus left for the Title or Tabs pane
/// without going through `workspace::switch_to` (a pure focus move touches
/// no document) — restores `app.active` to the document the user was
/// editing before this browsing session's preview took over
/// (`Explorer::preview_return_to`) when the preview WAS the active
/// document, since nothing else is about to reseat it the way
/// `switch_to`'s own caller does for [`discard_if_switching_away`]. Falls
/// back to `workspace::close::neighbor_of`'s adjacent-tab pick — reused
/// rather than a second neighbour picker — when the remembered document
/// has itself been closed in the meantime. The target is resolved BEFORE
/// `id` is removed: `neighbor_of` reads `id`'s own position in
/// `tabs.order`, exactly like `close_now`'s own reassign-before-remove
/// order.
fn discard_active(app: &mut App) {
    let Some(id) = app.explorer.preview else {
        return;
    };
    let was_active = app.active == id;
    let target = was_active
        .then(|| {
            app.explorer
                .preview_return_to
                .filter(|&t| t != id && app.documents.contains_key(&t))
                .or_else(|| workspace::close::neighbor_of(app, id))
        })
        .flatten();
    remove_preview_document(app, id);
    if let Some(target) = target {
        workspace::switch_to(app, target);
    }
    app.tabs.nav.cursor = app
        .tabs
        .order
        .iter()
        .position(|&t| t == app.active)
        .unwrap_or(0);
}

/// The shared tail of every discard path: removes `id` from BOTH
/// `app.documents` and `app.tabs.order` and clears the preview slot (and
/// its remembered return-to document). A preview is never dirty and never
/// contacted the recovery store, so unlike `workspace::close::close_now`
/// there is no guard prompt, no `db_ops`/`pending_*` sweep, and no
/// image-delete effect to run — closing a preview is pure bookkeeping.
fn remove_preview_document(app: &mut App, id: DocumentId) {
    app.documents.remove(&id);
    app.tabs.order.retain(|&t| t != id);
    app.explorer.preview = None;
    app.explorer.preview_return_to = None;
}

#[cfg(test)]
mod tests;
