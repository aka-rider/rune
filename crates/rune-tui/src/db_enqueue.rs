//! The enqueue side of the [`crate::db`] bridge (split out of `db.rs` to
//! keep it under the §1.6 line budget): the small functions the three
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
/// site. `local_pos` is `doc.journal.pos()` AFTER that push. A failure here
/// (enqueue-time `Error`, never an async one — that lands via `Msg::Db`
/// instead) only ever marks the whole store degraded
/// (`app::on_store_failure`) — the buffer/journal mutation already
/// happened and is never rolled back (plan decision 3). Every successful
/// enqueue records `id` in `app.db_ops` (plan decision 6) so the eventual
/// ack routes back to the right document.
pub fn append_edit(
    app: &mut App,
    id: DocumentId,
    local_pos: usize,
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
            if let Some(doc_db) = app.doc_mut(id).and_then(|d| d.db.as_mut()) {
                doc_db.note_pending_append(local_pos);
            }
        }
        Err(e) => crate::materialize_ack::on_store_failure(app, e.to_string()),
    }
}

/// Enqueues a `MoveUndoPos` replica of an undo/redo `id` just committed
/// locally (plan WP5.S3) — called immediately after `Journal::move_pos` at
/// `commands::edit::undo`/`redo`'s call sites. `local_pos` is the journal
/// position just committed (`Journal::move_pos`'s own argument).
pub fn move_undo_pos(app: &mut App, id: DocumentId, local_pos: usize) {
    if app.db.as_ref().is_none_or(|db| db.degraded) {
        return;
    }
    let Some(doc) = app.doc(id) else { return };
    let Some((target_seq, db_id)) = doc
        .db
        .as_ref()
        .map(|d| (d.seq_for_local_pos(local_pos), d.db_id))
    else {
        return;
    };
    let Some(db) = app.db.as_ref() else { return };
    let result = db.store.move_undo_pos(db_id, target_seq);
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
/// docs). A degraded store enqueues nothing — there is no trustworthy
/// recovery journal to bind this document to either way.
pub fn load_document(app: &mut App, id: DocumentId, path: &Path) {
    if app.db.as_ref().is_none_or(|db| db.degraded) {
        return;
    }
    let Some(doc) = app.doc(id) else { return };
    let issued_version = doc.buffer.version();
    let Some(db) = app.db.as_ref() else { return };
    match db.store.load(path) {
        Ok(op_id) => {
            app.db_ops
                .insert(op_id, PendingOp::load(id, issued_version));
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
