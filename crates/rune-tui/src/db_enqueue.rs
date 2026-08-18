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
use crate::document::{DocumentId, Replica, ReplicaStep};

/// THE sole chokepoint an edit batch's replica reaches after `Journal::
/// push` at `commands::edit_core::apply_edit_batch_with_cursors`'s one call
/// site: what happens next depends on `id`'s
/// [`Replica`]. `Detached` (no store, a degraded one, or a document with no
/// recovery journal) does nothing — same as always. `Binding` (a `Load`/
/// `CreateScratch` op is still in flight) buffers the batch as a
/// [`ReplicaStep`] instead of dropping it — [`crate::db_ack::install_doc_db`]
/// replays every buffered step, in order, as a real `AppendEdit` the moment
/// the ack installs the `DocDb`, restoring the 1:1 correspondence between
/// local journal positions and durable `events` rows that silently dropping
/// a pre-bind edit used to break. `Bound` enqueues immediately, exactly as
/// before.
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
    let Some(doc) = app.doc_mut(id) else { return };
    if let Replica::Binding { pending, .. } = &mut doc.replica {
        pending.push(ReplicaStep::new(edits, cursors_before, cursors_after));
        return;
    }
    if !doc.replica.is_bound() {
        return;
    }
    append_edit_bound(app, id, edits, cursors_before, cursors_after);
}

/// The actual `AppendEdit` enqueue, shared by [`append_edit`]'s `Bound`
/// branch and [`replay_pending`]'s replay loop — a failure here (enqueue-
/// time `Error`, never an async one — that lands via `Msg::Db` instead)
/// only ever marks the whole store degraded (`materialize_ack::
/// on_store_failure`) — the buffer/journal mutation already happened and is
/// never rolled back. Every successful enqueue records `id` in
/// `app.db_ops` so the eventual ack routes back to the right document.
fn append_edit_bound(
    app: &mut App,
    id: DocumentId,
    edits: &[AppliedEdit],
    cursors_before: &[Cursor],
    cursors_after: &[Cursor],
) {
    flush_pending_rebase(app, id);
    send_append(app, id, edits, cursors_before, cursors_after);
}

/// Journals `id`'s deferred re-base bridge, if one is still pending
/// (`DocDb::pending_rebase`'s own doc comment) — called immediately before
/// the first op whose meaning depends on the bound row reconstructing to
/// the buffer: an `AppendEdit` ([`append_edit_bound`]), a durable undo
/// move ([`move_undo_pos`]), a save (`save::materialize`). Ops that only
/// READ the reconstruction (probe, merge prep) deliberately never flush:
/// until the user commits content through this binding, a dead session's
/// recovered-but-not-adopted draft must stay reconstructable.
pub(crate) fn flush_pending_rebase(app: &mut App, id: DocumentId) {
    let Some(step) = app
        .doc_mut(id)
        .and_then(crate::document::Document::doc_db_mut)
        .and_then(|db| db.pending_rebase.take())
    else {
        return;
    };
    send_append(
        app,
        id,
        &step.edits,
        &step.cursors_before,
        &step.cursors_after,
    );
}

fn send_append(
    app: &mut App,
    id: DocumentId,
    edits: &[AppliedEdit],
    cursors_before: &[Cursor],
    cursors_after: &[Cursor],
) {
    if app.db.as_ref().is_none_or(|db| db.degraded) {
        return;
    }
    let Some(doc) = app.doc(id) else { return };
    let Some(db_id) = doc.doc_db().map(|d| d.db_id) else {
        return;
    };
    let Some(db) = app.db.as_ref() else { return };
    let result = db
        .store
        .append_edit(rune_db::DocId(db_id), edits, cursors_before, cursors_after);
    match result {
        Ok(op_id) => {
            app.db_ops.insert(op_id, PendingOp::append(id));
            if let Some(doc_db) = app.doc_mut(id).and_then(|d| d.doc_db_mut()) {
                doc_db.appends_sent += 1;
            }
        }
        Err(e) => crate::materialize_ack::on_store_failure(app, &e.to_string()),
    }
}

/// The one place a local journal count enters the writer's `i64` position
/// arithmetic — a count past `i64::MAX` is unreachable for any real
/// journal, and saturating there keeps the arithmetic total without a
/// panic path.
pub(crate) fn journal_i64(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// Replays every [`ReplicaStep`] a `Binding` window buffered, in order, as a
/// real `AppendEdit` — called by [`crate::db_ack::install_doc_db`] right
/// after it moves `id`'s `Replica` to `Bound`, so every buffered step reaches
/// the store before this document is ever considered fully bound.
pub(crate) fn replay_pending(app: &mut App, id: DocumentId, pending: Vec<ReplicaStep>) {
    for step in pending {
        append_edit_bound(
            app,
            id,
            &step.edits,
            &step.cursors_before,
            &step.cursors_after,
        );
    }
}

/// Enqueues a `MoveUndoPos` replica of an undo/redo `id` just committed
/// locally — called immediately after `Journal::move_pos` at
/// `commands::edit::undo`/`redo`'s call sites. `local_pos` is the journal
/// position just committed (`Journal::move_pos`'s own argument), mapped
/// through `DocDb::undo_offset` into the writer thread's own numbering —
/// carried to the writer thread AS-IS from there, never further
/// resolved to a durable seq here: only the writer thread, which has
/// already executed every `AppendEdit` this session enqueued ahead of this
/// op, can resolve it exactly (see `rune_db::OpKind::MoveUndoPos`'s doc
/// comment). A position mapping outside the writer's own numbering —
/// below `DocDb::undo_floor` (a state that predates a rebind's re-base
/// bridge) or above `DocDb::appends_sent` (an entry the writer never ran)
/// — cannot be sent as an exact position: mis-resolving it lands
/// `current_seq` on another lineage's seq and recovery silently truncates
/// or resurrects content. Such a move is journaled as a FORWARD re-base
/// instead: one replace-all `AppendEdit` from `pre_content` (the buffer as
/// it stood before this undo/redo committed, which is exactly what the
/// durable replica reconstructs to) to the post-move buffer, after which
/// the mapping re-anchors and later moves resolve exactly again. This op
/// is doc-scoped (`PendingOp::doc_scoped`): a resolution failure is a fact
/// about ONE document's undo position, never a reason to degrade the whole
/// store.
pub fn move_undo_pos(app: &mut App, id: DocumentId, local_pos: usize, pre_content: &str) {
    if app.db.as_ref().is_none_or(|db| db.degraded) {
        return;
    }
    if app
        .doc(id)
        .and_then(crate::document::Document::doc_db)
        .is_none()
    {
        return;
    }
    flush_pending_rebase(app, id);
    let Some(doc) = app.doc(id) else { return };
    let Some(doc_db) = doc.doc_db() else { return };
    let db_id = doc_db.db_id;
    let resolved_pos = journal_i64(local_pos) - doc_db.undo_offset;
    if resolved_pos < doc_db.undo_floor || resolved_pos > doc_db.appends_sent {
        rebase_move(app, id, pre_content);
        return;
    }
    let Some(db) = app.db.as_ref() else { return };
    let result = db.store.move_undo_pos(rune_db::DocId(db_id), resolved_pos);
    match result {
        Ok(op_id) => {
            app.db_ops.insert(op_id, PendingOp::move_undo_pos(id));
        }
        Err(e) => crate::materialize_ack::on_store_failure(app, &e.to_string()),
    }
}

/// Journals an undo/redo the writer's numbering cannot express as one
/// forward replace-all `AppendEdit` (`move_undo_pos`'s own doc comment),
/// then re-anchors the mapping on the position the replica now sits at:
/// every position at or above it resolves exactly from here on, and any
/// later move back below lands here again.
fn rebase_move(app: &mut App, id: DocumentId, pre_content: &str) {
    let current = match app.doc(id) {
        Some(doc) => doc.buffer.content().to_string(),
        None => return,
    };
    if pre_content != current {
        send_append(
            app,
            id,
            &[AppliedEdit {
                start: 0,
                end: pre_content.len(),
                deleted: pre_content.to_string(),
                insert: current,
            }],
            &[],
            &[],
        );
    }
    let Some(doc) = app.doc_mut(id) else { return };
    let pos = journal_i64(doc.journal.pos());
    let Some(doc_db) = doc.doc_db_mut() else {
        return;
    };
    doc_db.undo_offset = pos - doc_db.appends_sent;
    doc_db.undo_floor = doc_db.appends_sent;
}

/// Enqueues a `Load` op hydrating `id` (already bound to `path`, an
/// existing file just read straight off disk — `workspace::open_path`'s one
/// call site) through the app-wide recovery store, closing the "Explorer-
/// opened documents get no recovery journal" gap. Records `id`'s
/// buffer version at the moment the load is ISSUED, alongside the routing
/// entry, in one `PendingOp` in `app.db_ops` — `app::handle_db_event`'s
/// `Load` arm needs both to decide, on the ack, whether adopting the
/// recovered content is still safe (see [`crate::db_ack::handle_load_ack`]'s
/// docs). `intent` becomes that `PendingOp`'s `binding_only` flag — see
/// `PendingOp::binding_only`'s own doc comment for why a `Rebaseline` call
/// must set it. A degraded store enqueues nothing — there is no
/// trustworthy recovery journal to bind this document to either way.
///
/// Returns whether the op was actually enqueued: a re-baseline caller
/// (`materialize_ack::reactions`) must drop `id`'s existing `db` binding
/// on `false` rather than leave it standing with a baseline it can no
/// longer refresh; the other call sites, which have no binding yet to
/// protect, are unaffected either way.
pub fn load_document(app: &mut App, id: DocumentId, path: &Path, intent: LoadIntent) -> bool {
    load_document_inner(app, id, path, intent, LoadErrorPolicy::Degrade)
}

/// The re-baseline counterpart to [`load_document`]: same enqueue, but an
/// error NEVER calls `on_store_failure` — used only by the committed-outcome
/// re-baseline in `materialize_ack::reactions`, which may run once per
/// document inside a `DbEvent::Fatal` teardown loop still mid-flight over
/// OTHER documents' own saves. Degrading the store from inside that loop
/// would let `on_store_failure`'s in-flight sweep drop a LATER document's
/// still-queued `save_pending` before its own synthetic ack even lands —
/// the re-baseline is best-effort bookkeeping and must never tear the whole
/// world down. The caller already treats a `false` return as "drop this
/// document's binding, the next save falls back to direct-vfs", and the
/// outer `Fatal` arm reports the store failure itself once the whole loop
/// has finished.
pub fn load_document_best_effort(app: &mut App, id: DocumentId, path: &Path) -> bool {
    load_document_inner(
        app,
        id,
        path,
        LoadIntent::Rebaseline,
        LoadErrorPolicy::Ignore,
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LoadIntent {
    Recover,
    Rebaseline,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LoadErrorPolicy {
    Degrade,
    Ignore,
}

fn load_document_inner(
    app: &mut App,
    id: DocumentId,
    path: &Path,
    intent: LoadIntent,
    on_err: LoadErrorPolicy,
) -> bool {
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
            app.db_ops.insert(
                op_id,
                PendingOp::load(id, issued_version, intent == LoadIntent::Rebaseline),
            );
            // A document with no binding yet starts buffering: every edit
            // committed before this op's ack lands must reach the store
            // eventually, not be dropped. A re-baseline/hand-off
            // `Load` (`load_document_best_effort`, or a `Rebaseline` call
            // against an already-`Bound` document) targets a document that
            // is already `Bound` or `Detached`-by-design, so this is a no-op
            // for those — only a genuinely fresh, never-bound document
            // transitions here.
            if let Some(doc) = app.doc_mut(id)
                && matches!(doc.replica, Replica::Detached)
            {
                doc.replica = Replica::Binding {
                    base: doc.buffer.content().to_string(),
                    pending: Vec::new(),
                };
            }
            true
        }
        Err(e) => {
            if on_err == LoadErrorPolicy::Degrade {
                crate::materialize_ack::on_store_failure(app, &e.to_string());
            }
            false
        }
    }
}

/// Enqueues a `Probe` op refreshing `id`'s disk fact — called
/// from `workspace::switch_to` for a document with both a `db` binding and a
/// `file_path` (nothing to probe for a pathless draft or one with no
/// recovery journal). Skips enqueueing if `id` already has a probe in
/// flight (`PendingOp::is_probe`, `db.rs`'s own doc comment) — a rapid
/// sequence of tab switches back onto the same document must not stack
/// redundant probes. The resulting `SyncState` lands as `OpOutcome::Sync`,
/// handled in `db_dispatch::handle_db_event`.
///
/// Deferred instead of enqueued while ANY document bound to the same
/// `db_id` has a save in flight — a save from one tab invalidates the disk
/// for every tab open on that file, not only the one saving, so a probe
/// issued now would only read the pre-save world and get
/// dropped by the epoch check its ack lands into anyway (`db_dispatch`'s
/// `OpOutcome::Sync` arm). The shared `FileBinding::pending_probe` records
/// the request instead; `materialize_ack::handle_materialize_ack`'s tail
/// re-calls this function, for every document still bound to `db_id`, once
/// ANY of their saves resolves — so the disk fact each of them ends up with
/// is read fresh from the POST-save world, exactly once.
pub fn probe(app: &mut App, id: DocumentId) {
    if app.db.as_ref().is_none_or(|db| db.degraded) {
        return;
    }
    let Some(doc) = app.doc(id) else { return };
    let Some(db_id) = doc.doc_db().map(|d| d.db_id) else {
        return;
    };
    if doc.file_path.is_none() {
        return;
    }
    if app.any_save_in_flight_for(db_id) {
        if let Some(binding) = app.file_binding_mut(db_id) {
            binding.pending_probe = true;
        }
        return;
    }
    if app
        .db_ops
        .values()
        .any(|pending| pending.doc == id && pending.is_probe)
    {
        return;
    }
    let baseline_epoch = app.file_binding(db_id).map_or(0, |b| b.baseline_epoch);
    let Some(db) = app.db.as_ref() else { return };
    match db.store.probe(rune_db::DocId(db_id)) {
        Ok(op_id) => {
            app.db_ops
                .insert(op_id, PendingOp::probe(id, baseline_epoch));
        }
        Err(e) => crate::materialize_ack::on_store_failure(app, &e.to_string()),
    }
}

/// Enqueues a `CreateScratch` op registering `id` — a freshly minted
/// untitled draft (`workspace::new_untitled_document`'s one call site) — as
/// its own scratch row in the recovery store, closing the "an untitled
/// draft minted mid-session has no journal" gap. Mirrors
/// `load_document`'s enqueue-then-record shape: a degraded or absent store
/// enqueues nothing, leaving the document `Detached` and the draft exactly
/// as unpreserved as it is today — `App::is_preserved` already reports that
/// honestly, so there is nothing else to gate here. The ack binds the
/// document's `DocDb` (`db_ack::handle_create_scratch_ack`); until it lands
/// the draft is simply not yet preserved, same as any other in-flight
/// recovery op.
pub fn create_scratch(app: &mut App, id: DocumentId) {
    if app.db.as_ref().is_none_or(|db| db.degraded) {
        return;
    }
    let Some(db) = app.db.as_ref() else { return };
    match db.store.create_scratch() {
        Ok(op_id) => {
            app.db_ops.insert(op_id, PendingOp::new(id));
            if let Some(doc) = app.doc_mut(id)
                && matches!(doc.replica, Replica::Detached)
            {
                doc.replica = Replica::Binding {
                    base: doc.buffer.content().to_string(),
                    pending: Vec::new(),
                };
            }
        }
        Err(e) => crate::materialize_ack::on_store_failure(app, &e.to_string()),
    }
}
