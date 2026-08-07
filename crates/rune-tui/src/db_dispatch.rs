//! The `rune-db` ack router, split out of `dispatch.rs` (500-line budget): a
//! distinct concern from the `Msg` dispatch and key pipeline that file keeps
//! — this is reached only through `dispatch::update_inner`'s `Msg::Db` arm.

use crate::app::App;
use crate::document::DocumentId;
use crate::materialize_ack;
use crate::runtime::Effects;
use rune_db::DbEvent;

/// Routes a `rune-db` writer-thread completion via `App::db_ops`: the ack's
/// own op id is popped from `db_ops` to find which `DocumentId` enqueued
/// it; an id with no entry (already resolved, or from a `Load` op handled
/// during bootstrap hydration instead — see `db::DbBridge`'s doc comment)
/// is ignored. Only `Materialize` acks (the save path), `AppendEdit` acks
/// (seq bookkeeping, `db::resolve_append_ack`), and `Load` acks
/// (per-document hydration, `db::handle_load_ack`) need a per-document
/// reaction on success; `MoveUndoPos`/`CreateSnapshot`/adoption acks are
/// fire-and-forget. Any `Err`/`Fatal` degrades the WHOLE store — never a
/// buffer rollback.
pub(crate) fn handle_db_event(app: &mut App, evt: DbEvent, effects: &mut Effects) {
    match evt {
        DbEvent::Ok {
            id: op_id,
            result: rune_db::OpOutcome::Seq(seq),
        } => {
            if let Some(pending) = app.db_ops.remove(&op_id) {
                crate::db_ack::resolve_append_ack(app, pending.doc, seq);
            }
        }
        DbEvent::Ok {
            id: op_id,
            result: rune_db::OpOutcome::MaterializePrep(prep),
        } => {
            if let Some(pending) = app.db_ops.remove(&op_id) {
                materialize_ack::handle_prepare_ack(app, pending.doc, *prep, effects);
            }
        }
        DbEvent::Ok {
            id: op_id,
            result: rune_db::OpOutcome::Materialize(mat),
        } => {
            app.published_ops.remove(&op_id);
            if let Some(pending) = app.db_ops.remove(&op_id) {
                materialize_ack::handle_materialize_ack(app, pending.doc, *mat);
            }
        }
        DbEvent::Ok {
            id: op_id,
            result: rune_db::OpOutcome::Rename(outcome),
        } => {
            app.db_ops.remove(&op_id);
            crate::rename::handle_rename_ack(app, op_id, *outcome, effects);
        }
        DbEvent::Ok {
            id: op_id,
            result: rune_db::OpOutcome::Load(load_result),
        } => {
            if let Some(pending) = app.db_ops.remove(&op_id) {
                crate::db_ack::handle_load_ack(
                    app,
                    pending.doc,
                    *load_result,
                    pending.issued_version,
                    pending.binding_only,
                );
            }
        }
        DbEvent::Ok {
            id: op_id,
            result: rune_db::OpOutcome::Sync(state),
        } => {
            // A `Probe` ack — render/hint state only (see
            // `Document::last_sync`'s own doc comment). Updating it here, in
            // the same dispatch the ack lands in, is what keeps the
            // footer's `DiskChanged` hint from ever needing its own
            // after-update reconciler (a message always arrives to drive
            // this — the known "reconcilers only run when a message
            // arrives" gap doesn't apply).
            let Some(pending) = app.db_ops.remove(&op_id) else {
                return;
            };
            let current_epoch = app
                .doc(pending.doc)
                .and_then(|d| d.db.as_ref().map(|db| db.save_epoch));
            if pending.probe_epoch != current_epoch {
                // Stale: a materialize publish landed between this probe's
                // issue and its own ack — the disk fact it carries no
                // longer describes the current world, so it is dropped
                // rather than trusted (mirrors the `MergePrep` ticket check
                // below).
                return;
            }
            if let Some(doc) = app.doc_mut(pending.doc) {
                doc.last_sync = Some(state.kind);
            }
            // Two self-retractions ride the same confirmed classification:
            // a `DiskConflict` Guard raised by an earlier save's CAS
            // mismatch, and an `Active` merge nothing has been resolved on
            // yet — both exist only because disk once looked diverged, and
            // both should let go the moment a later probe says it no
            // longer does.
            crate::guard::retract_disk_conflict_on_convergence(app, pending.doc, state.kind);
            crate::merge::retract_active_on_convergence(app, pending.doc, state.kind);
        }
        DbEvent::Ok {
            id: op_id,
            result: rune_db::OpOutcome::MergePrep(prep),
        } => {
            // The fresh-state read `merge::begin` kicked off —
            // `pending.merge_gen` is this attempt's own ticket, checked
            // against `App.merge`'s CURRENT `Pending` generation inside the
            // landing handler itself (a later `^M` may have superseded it).
            if let Some(pending) = app.db_ops.remove(&op_id) {
                crate::merge::handle_merge_prep_ack(
                    app,
                    pending.doc,
                    pending.merge_gen,
                    *prep,
                    effects,
                );
            }
        }
        DbEvent::Ok {
            id: op_id,
            result: rune_db::OpOutcome::RowId(row_id),
        } => {
            // `RowId` is shared by two op kinds (`db::PendingOp::mints_
            // scratch`'s own doc comment): a `CreateScratch` minting a
            // mid-session untitled draft's own recovery row binds a fresh
            // `DocDb`; a `CreateSnapshot` anchor
            // (`materialize_ack::handle_snapshot_due`) is fire-and-forget —
            // popping it from `db_ops` here is its only needed reaction.
            if let Some(pending) = app.db_ops.remove(&op_id)
                && pending.mints_scratch
            {
                crate::db_ack::handle_create_scratch_ack(app, pending.doc, row_id);
            }
        }
        DbEvent::Ok { id: op_id, .. } => {
            app.db_ops.remove(&op_id);
            // A `TouchSearchQuery` ack (`OpOutcome::None`) lands
            // here — nothing else to react to, just retire the tracking
            // entry `search::keys::persist_query` inserted at enqueue, so
            // it can't be mistaken for still-in-flight by a later `Err` for
            // an unrelated op that happens to reuse the id space.
            app.search_history_ops.remove(&op_id);
        }
        DbEvent::Err { id: op_id, error } => {
            app.db_ops.remove(&op_id);
            // A cosmetic `search_history` write failing must never
            // sticky-degrade the whole recovery store the way a real
            // journal/materialize failure does — the bar keeps working,
            // only the just-used query wasn't recorded.
            if app.search_history_ops.remove(&op_id) {
                crate::messages::error(app, format!("search history not saved: {error}"));
                return;
            }
            crate::rename::fail_op(app, op_id, error.clone(), effects);
            // This exact op may be a `MaterializeRecord` whose disk
            // write ALREADY physically completed before the writer died —
            // report the save as successful FIRST, so `on_store_failure`'s
            // in-flight sweep below never re-flags it as failed.
            if let Some(doc_id) = app.published_ops.remove(&op_id) {
                materialize_ack::handle_materialize_ack(
                    app,
                    doc_id,
                    rune_db::MatResult {
                        committed: true,
                        ..Default::default()
                    },
                );
            }
            materialize_ack::on_store_failure(app, error);
        }
        DbEvent::Fatal { error } => {
            crate::rename::fail_all(app, error.clone(), effects);
            // Same reasoning as the `Err` arm above, for every
            // still-in-flight `MaterializeRecord` a `Fatal` tears down —
            // each one's write already physically completed.
            let published: Vec<DocumentId> = app.published_ops.drain().map(|(_, id)| id).collect();
            for doc_id in published {
                materialize_ack::handle_materialize_ack(
                    app,
                    doc_id,
                    rune_db::MatResult {
                        committed: true,
                        ..Default::default()
                    },
                );
            }
            materialize_ack::on_store_failure(app, error);
            // Degraded mode gates every FUTURE enqueue (`db::append_edit`/
            // `move_undo_pos`/`save::materialize_now`/`handle_snapshot_due`
            // all bail out once `db.degraded`), but does nothing about
            // in-flight entries already sitting in `db_ops` — a `Fatal`
            // tears the whole writer thread down, so none of them will
            // EVER receive their ack. Left alone, they'd carry dead weight
            // forward for the rest of the session (an unbounded leak across
            // a long-running degrade-then-keep-editing session); clearing
            // them here is correct, not just tidy — `App::doc_mut` already
            // treats a missing `db_ops` entry as a plain no-op for any
            // ack that *did* somehow still land, so no real ack is ever
            // silently dropped by this.
            app.db_ops.clear();
        }
    }
}
