//! The `rune-db` ack router, split out of `dispatch.rs` (§1.6 budget): a
//! distinct concern from the `Msg` dispatch and key pipeline that file keeps
//! — this is reached only through `dispatch::update_inner`'s `Msg::Db` arm.

use crate::app::App;
use crate::document::DocumentId;
use crate::materialize_ack;
use crate::runtime::Effects;
use rune_db::DbEvent;

/// Routes a `rune-db` writer-thread completion (plan WP5.S1, re-routed in
/// WP1 via `App::db_ops` — plan decision 6): the ack's own op id is popped
/// from `db_ops` to find which `DocumentId` enqueued it; an id with no
/// entry (already resolved, or from a `Load` op handled during bootstrap
/// hydration instead — see `db::DbBridge`'s doc comment) is ignored. Only
/// `Materialize` acks (the save path, WP5.S6), `AppendEdit` acks (seq
/// bookkeeping, `db::resolve_append_ack`), and `Load` acks (per-document
/// hydration, `db::handle_load_ack`) need a per-document reaction on
/// success; `MoveUndoPos`/`CreateSnapshot`/adoption acks are fire-and-
/// forget. Any `Err`/`Fatal` degrades the WHOLE store (plan decision 3) —
/// never a buffer rollback.
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
                );
            }
        }
        DbEvent::Ok { id: op_id, .. } => {
            app.db_ops.remove(&op_id);
        }
        DbEvent::Err { id: op_id, error } => {
            app.db_ops.remove(&op_id);
            crate::rename::fail_op(app, op_id, error.clone(), effects);
            // WP7: this exact op may be a `MaterializeRecord` whose disk
            // write ALREADY physically completed before the writer died —
            // report the save as successful FIRST, so `on_store_failure`'s
            // in-flight sweep below never re-flags it as failed
            // ([rune-db 1]).
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
            // WP7: same reasoning as the `Err` arm above, for every
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
