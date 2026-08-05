//! `request_close`/`close_now` — the tab-close chokepoint (split out of
//! `workspace` per §1.6; WP5.S7 adds the image-delete-on-close hook, which
//! needed an `Effects` sink `close_now` didn't carry before).

use crate::app::App;
use crate::banner::{self, GuardKind, GuardPrompt, Modal};
use crate::document::DocumentId;
use crate::runtime::Effects;

/// The result of [`close_now`]: `Unknown` when `id` was already stale (a
/// racing close, or a never-live id) — the bare early return this replaces
/// used to make that indistinguishable from an actual close from the
/// caller's side, which is exactly the "no caller can silently do nothing"
/// gap this enum closes.
#[must_use]
pub enum CloseOutcome {
    Closed,
    Unknown,
}

/// Requests closing `id` (plan WP5.S3): closes immediately if `id` is
/// clean, or arms the close-guard modal if it's dirty. A stale/already-
/// closed `id` is a silent no-op. Closing the LAST remaining document is no
/// longer refused — `close_now` mints a fresh untitled draft to replace it.
pub fn request_close(app: &mut App, id: DocumentId, effects: &mut Effects) {
    if app.doc(id).is_none() {
        return;
    }
    // A not-yet-committed preview refuses `^W` outright: tearing it down
    // (and `close_now`'s neighbor-reactivation with it) is not the user's
    // intent for a document they never opened.
    if app.refuse_if_preview(id) {
        return;
    }
    // Re-derived, not read from the cache (CONSTITUTION §1.4.8: close is a
    // transition) — a stale cache could wave a genuinely-dirty document
    // through, or arm the Guard for one that's actually clean.
    if crate::materialize_ack::is_dirty_now(app, id) {
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
        let _ = close_now(app, id, effects);
    }
}

/// Mints an empty, pathless draft (Go's `CreateUntitled`) with the next
/// unused "Untitled N" display name, opened through the existing
/// `App::open_document` constructor and activated — `switch_to` reseeds the
/// title field and the breadcrumb reads live off `active_doc` at render
/// time, so activating here is the one place both need to happen. The
/// single constructor for this shape: `close_now` calls it below when the
/// last document is about to close, and WP3's bootstrap adoption calls it
/// when there is nothing recoverable to adopt instead.
///
/// Registers `id` as its own scratch row in the recovery store (plan WP0/
/// WP3's mid-session gap) whenever a live store exists: `db_enqueue::
/// create_scratch` enqueues nothing and `doc.db` stays `None` — exactly
/// today's behaviour — when there is no store or it's `degraded`, and
/// `App::is_preserved` already reports that honestly to the quit/close
/// guard. Fired after activation so the new tab's own document already
/// exists for the eventual ack (`db_ack::handle_create_scratch_ack`) to
/// bind onto.
pub fn new_untitled_document(app: &mut App) -> DocumentId {
    let name = next_untitled_name(app);
    let id = app.open_document(rune_core::buffer::Buffer::new(""));
    if let Some(doc) = app.doc_mut(id) {
        doc.display_name = Some(name);
    }
    super::switch_to(app, id);
    crate::db_enqueue::create_scratch(app, id);
    id
}

/// The next unused "Untitled N" display name. `pub`: `rune-cli`'s launch
/// bootstrap names a recovered draft's tab through this same chokepoint
/// rather than re-deriving the numbering scheme, and a second scheme is
/// exactly how two "Untitled 1"s end up open at once.
pub fn next_untitled_name(app: &App) -> String {
    format!("Untitled {}", next_untitled_number(app))
}

/// The next unused "Untitled N" suffix: one past the highest N already in
/// use among live documents, or 1 if none are. Scans `display_name` rather
/// than keeping a counter so a closed "Untitled 2" frees its number back up
/// — matching how Go's untitled numbering already behaves.
fn next_untitled_number(app: &App) -> usize {
    app.documents
        .values()
        .filter_map(|doc| doc.display_name.as_deref())
        .filter_map(|name| name.strip_prefix("Untitled ")?.parse::<usize>().ok())
        .max()
        .unwrap_or(0)
        + 1
}

/// Closes `id` unconditionally — the plan WP5.S3 chokepoint every close
/// path (clean `request_close`, the Guard's `[D]iscard`, and its `[S]ave`
/// once the save ack lands) funnels through. Reassigns `active` to a
/// neighbor FIRST when `id` is the active document — per the WP1 invariant
/// comment on `App::active_doc`/`active_doc_mut`, `active` must always
/// reference a live entry, so the reassignment happens before `id` is
/// removed, never after. Closing the LAST document is no longer refused
/// (plan WP0): a fresh untitled draft is minted and activated first, so
/// that same non-empty floor holds even transiently. Sweeps `db_ops` of any
/// entry still pointing at `id` — a stale ack would
/// already be a correct no-op via `App::doc_mut` returning `None` (see its
/// docs), but leaving the entry forever would make `db_ops` an unbounded
/// leak over a long session of open/close cycles. Because each entry is one
/// `PendingOp` carrying both the routing fact and (for a `Load` op) the
/// issued-version fact, this single sweep drops both together — there is no
/// second map that could still be holding a leaked version. Clears `pending_close_on_
/// save`/`pending_save_confirm` when either still targets `id` (review fix
/// for the latter — it was left dangling): both are doc-tagged `Option`s
/// that would otherwise point at a document that no longer exists, e.g. a
/// stray `SaveConfirmTimeout` generation match resurrecting a confirm gate
/// for a doc `[D]iscard` just closed.
///
/// `effects` (plan WP5.S7): before the document is removed, an open image
/// document's allocated Kitty id is deleted from the terminal — gated on
/// `app.graphics.kitty`, or a non-graphics terminal would receive escape
/// bytes it never asked for. `materialize_ack::close_if_pending` (the only
/// one of `close_now`'s three call sites with no `Effects` anywhere in its
/// own call chain) passes a scratch one it discards — provably harmless,
/// since an image document is always `read_only` with `db: None`, so it
/// can never be dirty, never has `save_in_flight`, and `pending_close_on_
/// save` can never target one; this branch is therefore dead code on that
/// path, not a silently-dropped real delete. `banner`'s dirty-close
/// `[D]iscard` arm and `workspace::request_close` above both already carry
/// a real `Effects` and thread it straight through.
pub fn close_now(app: &mut App, id: DocumentId, effects: &mut Effects) -> CloseOutcome {
    if !app.documents.contains_key(&id) {
        return CloseOutcome::Unknown;
    }
    // Plan WP6.S3, decision 12: closing the merge document is an implicit
    // Esc — exit BEFORE `id` is removed below, or `app.merge` would go on
    // pointing at a document that no longer exists at all. `auto_exit`
    // (review fix F3) cancels a `Pending` attempt WITH feedback instead of
    // `exit_in_place` silently discarding it.
    if app.merge.doc() == Some(id) {
        crate::merge::auto_exit(app);
    }
    if app.graphics.kitty
        && let Some(image) = app.doc(id).and_then(|d| d.image.as_ref())
    {
        effects
            .raw
            .push(rune_image::encode_delete(image.id).into_bytes());
    }
    let mut active_changed = false;
    if app.documents.len() == 1 {
        // Closing the last document mints its replacement BEFORE `id` is
        // removed (Go's `CreateUntitled` parity, plan WP0): the non-empty
        // floor `App::active_doc`/`active_doc_mut` rely on is never
        // violated even transiently, and `new_untitled_document` already
        // activates the fresh draft and reseeds the title itself.
        new_untitled_document(app);
        active_changed = true;
    } else if app.active == id
        && let Some(neighbor) = neighbor_of(app, id)
    {
        app.active = neighbor;
        active_changed = true;
    }
    app.documents.remove(&id);
    app.tabs.order.retain(|&t| t != id);
    app.db_ops.retain(|_, pending| pending.doc != id);
    if app.pending_close_on_save == Some(id) {
        app.pending_close_on_save = None;
    }
    if app.pending_save_confirm.is_some_and(|(cid, _)| cid == id) {
        app.pending_save_confirm = None;
    }
    // A quit-save fan-out (plan WP2) may be waiting on `id` specifically —
    // its enqueued save will never ack once the document is gone, so
    // without this sweep the wait would strand forever instead of
    // resolving once every OTHER awaited document's save lands.
    crate::materialize_ack::retire_quit_wait(app, id);
    // The rename machine is one more doc-tagged pending slot to sweep
    // (plan's transition table: "any | close_now(doc) | Idle").
    crate::rename::forget_document(app, id);

    // `close_now` is the one active-document reseed with no blur in front of
    // it — reached from an async materialize ack and from the Guard's
    // `[D]iscard`, neither of which threads a REAL `Effects` here (see this
    // function's own doc comment on the scratch-`Effects` callers).
    //
    // Guarded on `active_changed` ALONE, deliberately. Closing some OTHER
    // document leaves `app.active` untouched and so cannot disturb a name
    // being typed. Closing the ACTIVE one must reseed even while the title
    // holds focus: the field would otherwise go on describing a document
    // that no longer exists while `app.active` has already moved to its
    // neighbour, and the next blur resolves the rename subject from
    // `app.active` — renaming that NEIGHBOUR to the name typed for the
    // closed document. Losing an in-progress name whose target just
    // vanished is the right trade; renaming a bystander is not.
    if active_changed {
        let name = crate::title::name_for(app.active_doc());
        app.title.seed(&name);
    }

    app.tabs.nav.cursor = app
        .tabs
        .order
        .iter()
        .position(|&t| t == app.active)
        .unwrap_or(0);
    CloseOutcome::Closed
}

/// The neighbor `close_now` reassigns `active` to when the closed document
/// WAS active: the next tab in `tabs.order`, else the previous one (plan
/// WP5.S3). Falls back to any other live document if `id` isn't in
/// `tabs.order` at all — shouldn't happen (every document has a tab), but
/// keeps this total rather than leaving `active` dangling.
///
/// `pub(crate)`: `explorer_preview::discard_active` reuses this exact pick
/// as its own fallback rather than growing a second neighbour picker.
pub(crate) fn neighbor_of(app: &App, id: DocumentId) -> Option<DocumentId> {
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
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use rune_core::buffer::Buffer;
    use rune_vfs::{Mem, Vfs};

    use crate::document::ReadOnly;

    use super::*;
    use crate::app::App;

    const X_PNG: &[u8] = include_bytes!("../../../../testdata/assets/x.png");

    /// Plan WP5.S7 Done-when: closing a `Live`-or-not image document pushes
    /// `encode_delete(id)` into `effects.raw` when the terminal is Kitty-
    /// capable, and never does when it isn't.
    #[test]
    fn closing_an_image_document_emits_encode_delete_when_kitty_is_on() {
        let mem = Arc::new(Mem::new());
        mem.save_atomic(Path::new("/vault/x.png"), X_PNG)
            .expect("seed x.png");
        let vfs: Arc<dyn Vfs + Send + Sync> = mem;
        let mut app = App::new(Buffer::new(""), None, vfs, None);
        app.graphics.kitty = true;
        let image_id =
            crate::workspace::open_path(&mut app, Path::new("/vault/x.png")).expect("open");
        let expected_id = app.doc(image_id).unwrap().image.as_ref().unwrap().id;

        let mut effects = Effects::default();
        let _ = close_now(&mut app, image_id, &mut effects);

        assert_eq!(effects.raw.len(), 1);
        assert_eq!(
            effects.raw[0],
            rune_image::encode_delete(expected_id).into_bytes()
        );
    }

    #[test]
    fn closing_an_image_document_emits_nothing_when_kitty_is_off() {
        let mem = Arc::new(Mem::new());
        mem.save_atomic(Path::new("/vault/x.png"), X_PNG)
            .expect("seed x.png");
        let vfs: Arc<dyn Vfs + Send + Sync> = mem;
        let mut app = App::new(Buffer::new(""), None, vfs, None);
        app.graphics.kitty = false;
        let image_id =
            crate::workspace::open_path(&mut app, Path::new("/vault/x.png")).expect("open");

        let mut effects = Effects::default();
        let _ = close_now(&mut app, image_id, &mut effects);

        assert!(effects.raw.is_empty());
    }

    #[test]
    fn closing_a_non_image_document_emits_nothing() {
        let mem = Arc::new(Mem::new());
        let vfs: Arc<dyn Vfs + Send + Sync> = mem;
        let mut app = App::new(Buffer::new("hello"), None, vfs, None);
        app.graphics.kitty = true;
        let extra = app.open_document(Buffer::new("second"));

        let mut effects = Effects::default();
        let _ = close_now(&mut app, extra, &mut effects);

        assert!(effects.raw.is_empty());
    }

    /// Plan WP0 Done-when: closing the ONLY open document does not refuse —
    /// it mints a fresh untitled draft, so the session always ends up with
    /// exactly one document and no "can't close" status.
    #[test]
    fn closing_the_only_document_mints_a_fresh_untitled_instead_of_refusing() {
        let mem = Arc::new(Mem::new());
        let vfs: Arc<dyn Vfs + Send + Sync> = mem;
        let mut app = App::new(Buffer::new("hello"), None, vfs, None);
        let only = app.active;

        let mut effects = Effects::default();
        let outcome = close_now(&mut app, only, &mut effects);

        assert!(matches!(outcome, CloseOutcome::Closed));
        assert_eq!(app.documents.len(), 1);
        assert!(!app.documents.contains_key(&only));
        assert_eq!(app.active_doc().display_name.as_deref(), Some("Untitled 1"));
        assert!(app.status_message.is_none());
    }

    #[test]
    fn request_close_refuses_a_preview_document() {
        let mem = Arc::new(Mem::new());
        let vfs: Arc<dyn Vfs + Send + Sync> = mem;
        let mut app = App::new(Buffer::new("hello"), None, vfs, None);
        let id = app.active;
        app.doc_mut(id).unwrap().read_only = ReadOnly::Preview;

        let mut effects = Effects::default();
        request_close(&mut app, id, &mut effects);

        assert!(
            app.documents.contains_key(&id),
            "a preview document must not be closed"
        );
        assert_eq!(app.active, id, "active must stay on the refused document");
        assert_eq!(
            app.status_message.as_deref(),
            ReadOnly::Preview.refusal_message()
        );
    }
}
