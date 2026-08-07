//! The enqueue side of the [`crate::db`] bridge (split out of `db.rs` to
//! keep it under the 500-line budget): the small functions the three
//! journal call sites (`commands::edit::commit_edit_batch`/`undo`/`redo`)
//! and `workspace::open_path` use to build and submit ops into `db_ops`.
//! The reaction to their eventual acks lives in [`crate::db_ack`].

use std::path::Path;

use rune_core::buffer::AppliedEdit;
use rune_core::cursor::Cursor;

use crate::app::App;
use crate::db::PendingOp;
use crate::document::DocumentId;

/// Enqueues an `AppendEdit` replica of a batch this session just committed
/// to `id`'s LOCAL in-memory journal (plan WP5.S3) — called immediately
/// after `Journal::push` at `commands::edit::commit_edit_batch`'s one call
/// site. A failure here (enqueue-time `Error`, never an async one — that
/// lands via `Msg::Db` instead) only ever marks the whole store degraded
/// (`app::on_store_failure`) — the buffer/journal mutation already
/// happened and is never rolled back (plan decision 3). Every successful
/// enqueue records `id` in `app.db_ops` (plan decision 6) so the eventual
/// ack routes back to the right document. The writer thread derives this
/// session's own local-position bookkeeping itself, from the ops it has
/// already run (`rune_db::OpKind::MoveUndoPos`'s doc comment) — this side
/// carries no local position of its own to track.
pub fn append_edit(
    app: &mut App,
    id: DocumentId,
    edits: &[AppliedEdit],
    cursors_before: &[Cursor],
    cursors_after: &[Cursor],
) {
    if app.db.as_ref().is_none_or(|db| db.degraded) {
        return;
    }
    // `id` not (or no longer) live is a plain, correct no-op — see
    // `App::doc`'s docs.
    let Some(doc) = app.doc(id) else { return };
    let Some(db_id) = doc.db.as_ref().map(|d| d.db_id) else {
        return;
    };
    let Some(db) = app.db.as_ref() else { return };
    let result = db
        .store
        .append_edit(db_id, edits, cursors_before, cursors_after);
    match result {
        Ok(op_id) => {
            app.db_ops.insert(op_id, PendingOp::new(id));
        }
        Err(e) => crate::materialize_ack::on_store_failure(app, e.to_string()),
    }
}

/// Enqueues a `MoveUndoPos` replica of an undo/redo `id` just committed
/// locally (plan WP5.S3) — called immediately after `Journal::move_pos` at
/// `commands::edit::undo`/`redo`'s call sites. `local_pos` is the journal
/// position just committed (`Journal::move_pos`'s own argument) — carried
/// to the writer thread AS-IS, never resolved to a durable seq here: only
/// the writer thread, which has already executed every `AppendEdit` this
/// session enqueued ahead of this op, can resolve it exactly (see
/// `rune_db::OpKind::MoveUndoPos`'s doc comment).
pub fn move_undo_pos(app: &mut App, id: DocumentId, local_pos: usize) {
    if app.db.as_ref().is_none_or(|db| db.degraded) {
        return;
    }
    let Some(doc) = app.doc(id) else { return };
    let Some(db_id) = doc.db.as_ref().map(|d| d.db_id) else {
        return;
    };
    let Some(db) = app.db.as_ref() else { return };
    let result = db.store.move_undo_pos(db_id, local_pos as i64);
    match result {
        Ok(op_id) => {
            app.db_ops.insert(op_id, PendingOp::new(id));
        }
        Err(e) => crate::materialize_ack::on_store_failure(app, e.to_string()),
    }
}

/// Enqueues a `Load` op hydrating `id` (already bound to `path`, an
/// existing file just read straight off disk — `workspace::open_path`'s one
/// call site) through the app-wide recovery store, closing the "Explorer-
/// opened documents get no recovery journal" gap (plan WP6). Records `id`'s
/// buffer version at the moment the load is ISSUED, alongside the routing
/// entry, in one `PendingOp` in `app.db_ops` — `app::handle_db_event`'s
/// `Load` arm needs both to decide, on the ack, whether adopting the
/// recovered content is still safe (see [`crate::db_ack::handle_load_ack`]'s
/// docs). `binding_only` is carried onto that `PendingOp` verbatim — see
/// `PendingOp::binding_only`'s own doc comment for why a re-baseline call
/// must set it. A degraded store enqueues nothing — there is no
/// trustworthy recovery journal to bind this document to either way.
///
/// Returns whether the op was actually enqueued: a re-baseline caller
/// (`materialize_ack::reactions`) must drop `id`'s existing `db` binding
/// on `false` rather than leave it standing with a baseline it can no
/// longer refresh; the other call sites, which have no binding yet to
/// protect, are unaffected either way.
pub fn load_document(app: &mut App, id: DocumentId, path: &Path, binding_only: bool) -> bool {
    if app.db.as_ref().is_none_or(|db| db.degraded) {
        return false;
    }
    let Some(doc) = app.doc(id) else { return false };
    let issued_version = doc.buffer.version();
    let Some(db) = app.db.as_ref() else {
        return false;
    };
    match db.store.load(path) {
        Ok(op_id) => {
            app.db_ops
                .insert(op_id, PendingOp::load(id, issued_version, binding_only));
            true
        }
        Err(e) => {
            crate::materialize_ack::on_store_failure(app, e.to_string());
            false
        }
    }
}

/// Enqueues a `Probe` op refreshing `id`'s disk fact (plan WP2.S4) — called
/// from `workspace::switch_to` for a document with both a `db` binding and a
/// `file_path` (nothing to probe for a pathless draft or one with no
/// recovery journal). Skips enqueueing if `id` already has a probe in
/// flight (`PendingOp::is_probe`, `db.rs`'s own doc comment) — a rapid
/// sequence of tab switches back onto the same document must not stack
/// redundant probes. The resulting `SyncState` lands as `OpOutcome::Sync`,
/// handled in `db_dispatch::handle_db_event`.
pub fn probe(app: &mut App, id: DocumentId) {
    if app.db.as_ref().is_none_or(|db| db.degraded) {
        return;
    }
    if app
        .db_ops
        .values()
        .any(|pending| pending.doc == id && pending.is_probe)
    {
        return;
    }
    let Some(doc) = app.doc(id) else { return };
    let Some(db_id) = doc.db.as_ref().map(|d| d.db_id) else {
        return;
    };
    if doc.file_path.is_none() {
        return;
    }
    let Some(db) = app.db.as_ref() else { return };
    match db.store.probe(db_id) {
        Ok(op_id) => {
            app.db_ops.insert(op_id, PendingOp::probe(id));
        }
        Err(e) => crate::materialize_ack::on_store_failure(app, e.to_string()),
    }
}

/// Enqueues a `CreateScratch` op registering `id` — a freshly minted
/// untitled draft (`workspace::new_untitled_document`'s one call site) — as
/// its own scratch row in the recovery store, closing the "an untitled
/// draft minted mid-session has no journal" gap (plan WP0/WP3). Mirrors
/// `load_document`'s enqueue-then-record shape: a degraded or absent store
/// enqueues nothing, leaving `doc.db` `None` and the draft exactly as
/// unpreserved as it is today — `App::is_preserved` already reports that
/// honestly, so there is nothing else to gate here. The ack binds `doc.db`
/// (`db_ack::handle_create_scratch_ack`); until it lands the draft is
/// simply not yet preserved, same as any other in-flight recovery op.
pub fn create_scratch(app: &mut App, id: DocumentId) {
    if app.db.as_ref().is_none_or(|db| db.degraded) {
        return;
    }
    let Some(db) = app.db.as_ref() else { return };
    match db.store.create_scratch() {
        Ok(op_id) => {
            app.db_ops.insert(op_id, PendingOp::create_scratch(id));
        }
        Err(e) => crate::materialize_ack::on_store_failure(app, e.to_string()),
    }
}
