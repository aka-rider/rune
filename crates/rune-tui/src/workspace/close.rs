//! `request_close`/`close_now` — the tab-close chokepoint (split out of
//! `workspace` per §1.6; WP5.S7 adds the image-delete-on-close hook, which
//! needed an `Effects` sink `close_now` didn't carry before).

use crate::app::{App, StatusSource};
use crate::banner::{self, GuardKind, GuardPrompt, Modal};
use crate::document::DocumentId;
use crate::runtime::Effects;

/// Requests closing `id` (plan WP5.S3): refuses outright if it's the LAST
/// remaining document (rune always shows one — the WP1 accessor floor on
/// `App::active_doc`/`active_doc_mut` depends on `documents` staying
/// non-empty), closes immediately if `id` is clean, or arms the close-guard
/// modal if it's dirty. A stale/already-closed `id` is a silent no-op.
pub fn request_close(app: &mut App, id: DocumentId, effects: &mut Effects) {
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
        close_now(app, id, effects);
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
pub fn close_now(app: &mut App, id: DocumentId, effects: &mut Effects) {
    if app.documents.len() <= 1 || !app.documents.contains_key(&id) {
        return;
    }
    if app.graphics.kitty
        && let Some(image) = app.doc(id).and_then(|d| d.image.as_ref())
    {
        effects
            .raw
            .push(rune_image::encode_delete(image.id).into_bytes());
    }
    let mut active_changed = false;
    if app.active == id
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
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use rune_core::buffer::Buffer;
    use rune_vfs::{Mem, Vfs};

    use super::*;
    use crate::app::App;

    const X_PNG: &[u8] = include_bytes!("../../../../golang/testdata/assets/x.png");

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
        close_now(&mut app, image_id, &mut effects);

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
        close_now(&mut app, image_id, &mut effects);

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
        close_now(&mut app, extra, &mut effects);

        assert!(effects.raw.is_empty());
    }
}
