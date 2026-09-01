use rune_core::buffer::AppliedEdit;
use rune_core::cursor::Cursor;
use rune_core::undo::EditKind;

use rune_db::EditBatch;

use crate::app::App;
use crate::db::{LoadPurpose, PendingOp};
use crate::document::{DocumentId, Replica, ReplicaStep};

// While a `Load`/`CreateScratch` op is still in flight (`Binding`), an edit
// batch is buffered as a `ReplicaStep` rather than dropped, and replayed in
// order once the binding completes: silently dropping a pre-bind edit would
// break the 1:1 correspondence between local journal positions and durable
// `events` rows.
pub fn append_edit(
    app: &mut App,
    id: DocumentId,
    edits: &[AppliedEdit],
    cursors_before: &[Cursor],
    cursors_after: &[Cursor],
    kind: EditKind,
) {
    if app.db.as_ref().is_none_or(|db| db.degraded) {
        return;
    }
    let Some(doc) = app.doc_mut(id) else { return };
    if let Replica::Binding { pending, .. } = &mut doc.replica {
        pending.push(ReplicaStep::new(edits, cursors_before, cursors_after, kind));
        return;
    }
    if !doc.replica.is_bound() {
        return;
    }
    append_edit_bound(app, id, edits, cursors_before, cursors_after, kind);
}

fn append_edit_bound(
    app: &mut App,
    id: DocumentId,
    edits: &[AppliedEdit],
    cursors_before: &[Cursor],
    cursors_after: &[Cursor],
    kind: EditKind,
) {
    flush_pending_rebase(app, id);
    send_append(app, id, edits, cursors_before, cursors_after, kind);
}

// Ops that only read the reconstruction (probe, merge prep) deliberately
// never flush: until the user commits content through this binding, a dead
// session's recovered-but-not-adopted draft must stay reconstructable.
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
        step.kind,
    );
}

fn send_append(
    app: &mut App,
    id: DocumentId,
    edits: &[AppliedEdit],
    cursors_before: &[Cursor],
    cursors_after: &[Cursor],
    kind: EditKind,
) {
    if app.db.as_ref().is_none_or(|db| db.degraded) {
        return;
    }
    let Some(doc) = app.doc(id) else { return };
    let Some(db_id) = doc.doc_db().map(|d| d.db_id) else {
        return;
    };
    let resolved_edits = resolve_drift(app, id, db_id, edits);
    let Some(doc) = app.doc(id) else { return };
    let Some(doc_db) = doc.doc_db() else { return };
    let token = doc_db.token;
    let token_base_seq = doc_db.token_base_seq;
    let Some(db) = app.db.as_ref() else { return };
    let result = db.store.append_edit(
        rune_db::DocId(db_id),
        token,
        token_base_seq,
        EditBatch {
            edits: &resolved_edits,
            cursors_before,
            cursors_after,
            kind,
        },
    );
    match result {
        Ok(op_id) => {
            app.db_ops.insert(op_id, PendingOp::append(id));
        }
        Err(e) => crate::materialize_ack::on_store_failure(app, &e.to_string()),
    }
}

// Detects and neutralizes drift between `id`'s own buffer coordinates and
// what `db_id`'s row actually, durably reconstructs to (a sibling binding's
// edits landing in between, or an external reload the buffer never
// adopted): a binding whose buffer no longer matches the shared truth must
// never journal an edit at coordinates computed against its own stale view,
// or the edit lands at the wrong offset in the row's real reconstruction.
//
// Cheap in the ordinary case (a lone, never-diverged binding) — no content
// clone happens at all. Once diverged, a pure single-edit insertion is
// re-targeted to the tail of the row's own current content (an "append
// what I just typed" merge — the only unambiguous translation without a
// real operational-transform engine); anything else falls back to one
// whole-content replace-all — content-correct always, fully surgical only
// when it can be.
fn resolve_drift(
    app: &mut App,
    id: DocumentId,
    db_id: i64,
    edits: &[AppliedEdit],
) -> Vec<AppliedEdit> {
    let needs_check = app
        .doc(id)
        .and_then(crate::document::Document::doc_db)
        .is_some_and(|d| d.diverged)
        || app.documents_bound_to(db_id).len() > 1;
    if !needs_check {
        return edits.to_vec();
    }

    let new_buffer_content = app
        .doc(id)
        .map(|d| d.buffer.content().to_string())
        .unwrap_or_default();
    let mut diverged = app
        .doc(id)
        .and_then(crate::document::Document::doc_db)
        .is_some_and(|d| d.diverged);
    let shared = app
        .file_binding(db_id)
        .map(|f| f.shared_content.clone())
        .unwrap_or_default();
    if !diverged {
        let synced = app
            .doc(id)
            .and_then(crate::document::Document::doc_db)
            .map(|d| d.synced_content.clone())
            .unwrap_or_default();
        diverged = synced != shared;
    }

    let (result, new_shared) = if !diverged {
        (edits.to_vec(), new_buffer_content.clone())
    } else if let [edit] = edits
        && edit.deleted.is_empty()
    {
        let pos = shared.len();
        let new_shared = format!("{shared}{}", edit.insert);
        (
            vec![AppliedEdit {
                start: pos,
                end: pos + edit.insert.len(),
                deleted: String::new(),
                insert: edit.insert.clone(),
            }],
            new_shared,
        )
    } else {
        (
            vec![AppliedEdit {
                start: 0,
                end: shared.len(),
                deleted: shared.clone(),
                insert: new_buffer_content.clone(),
            }],
            new_buffer_content.clone(),
        )
    };

    if let Some(doc_db) = app.doc_mut(id).and_then(|d| d.doc_db_mut()) {
        doc_db.diverged = diverged;
        doc_db.synced_content = new_shared.clone();
    }
    if let Some(fb) = app.file_binding_mut(db_id) {
        fb.shared_content = new_shared;
    }
    result
}

// A count past `i64::MAX` is unreachable for any real journal; saturating
// here keeps the arithmetic total without a panic path.
pub(crate) fn journal_i64(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

pub(crate) fn replay_pending(app: &mut App, id: DocumentId, pending: Vec<ReplicaStep>) {
    for step in pending {
        append_edit_bound(
            app,
            id,
            &step.edits,
            &step.cursors_before,
            &step.cursors_after,
            step.kind,
        );
    }
}

// `local_pos` is mapped through `DocDb::undo_offset` into `token`'s own
// numbering and carried to the writer thread as-is: only the writer, which
// has already executed every `AppendEdit` this token enqueued, can resolve
// it exactly. A position mapping below `DocDb::undo_floor` predates this
// token's numbering entirely and cannot be sent as an exact position —
// mis-resolving it would land `current_seq` on another lineage's seq and
// silently truncate or resurrect content on recovery. Such a move is
// journaled as a forward re-base instead (`rebase_move`).
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
    let token = doc_db.token;
    let token_base_seq = doc_db.token_base_seq;
    let resolved_pos = journal_i64(local_pos) - doc_db.undo_offset;
    if resolved_pos < doc_db.undo_floor {
        rebase_move(app, id, pre_content);
        return;
    }
    let Some(db) = app.db.as_ref() else { return };
    let result = db
        .store
        .move_undo_pos(rune_db::DocId(db_id), token, token_base_seq, resolved_pos);
    match result {
        Ok(op_id) => {
            app.db_ops.insert(op_id, PendingOp::move_undo_pos(id));
        }
        Err(e) => crate::materialize_ack::on_store_failure(app, &e.to_string()),
    }
}

// Mints a fresh `BindingToken` for `id`'s binding — the same "start
// numbering over" a rebind performs — sends the bridge as that token's own
// first append, then re-anchors `undo_offset`/`undo_floor` on the position
// the replica now sits at: every position at or above it resolves exactly
// from here on under the new token, and any later move back below lands
// here again.
fn rebase_move(app: &mut App, id: DocumentId, pre_content: &str) {
    let current = match app.doc(id) {
        Some(doc) => doc.buffer.content().to_string(),
        None => return,
    };
    let bridged = pre_content != current;
    let Some(doc_db) = app.doc_mut(id).and_then(|d| d.doc_db_mut()) else {
        return;
    };
    doc_db.token = rune_db::BindingToken::next();
    doc_db.token_base_seq = doc_db.last_known_seq;
    doc_db.diverged = false;
    doc_db.synced_content = pre_content.to_string();
    if bridged {
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
            EditKind::Other,
        );
    }
    let Some(doc) = app.doc_mut(id) else { return };
    let pos = journal_i64(doc.journal.pos());
    let Some(doc_db) = doc.doc_db_mut() else {
        return;
    };
    doc_db.undo_floor = i64::from(bridged);
    doc_db.undo_offset = pos - doc_db.undo_floor;
}

#[path = "db_enqueue_load.rs"]
mod db_enqueue_load;
pub use db_enqueue_load::{
    LoadIntent, create_scratch, load_document, load_document_best_effort, probe,
};
