//! The ack-reaction side of the [`crate::db`] bridge (split out of `db.rs`
//! to keep it under the §1.6 line budget): reacting to a `Load` op's ack and
//! to an `AppendEdit` ack's durable seq. [`crate::db_enqueue`] owns building
//! and submitting the ops these react to.

use crate::app::{App, StatusSource};
use crate::db::DocDb;
use crate::document::DocumentId;
use rune_db::LoadResult;

/// The reaction to a `Load` op's ack (plan WP6.S2/S3) — routed from
/// `app::handle_db_event` once `app.db_ops` has resolved the ack's op id to
/// `id`. `issued_version` is `id`'s buffer version recorded by
/// `db_enqueue::load_document` at ENQUEUE time, on the same `PendingOp` that
/// resolved `id`, `None` only if this ack's routing entry was somehow
/// already consumed.
///
/// A `None` `saved_obs` (should not occur — see `LoadResult::saved_obs`'s
/// own doc comment) installs nothing and surfaces a status message instead
/// of binding a document to a recovery row with no CAS baseline.
///
/// Otherwise, `recovered` is adopted into the buffer, through
/// [`crate::document::Document::hydrate`], ONLY when `issued_version` still
/// equals the buffer's CURRENT version — `Load` is asynchronous, so the
/// user may have typed into the buffer during the round trip, and
/// clobbering those keystrokes to complete a recovery binding would violate
/// the Prime Directive. When the version has moved on, `DocDb` is still
/// installed (this document's own recovery journal is real and should be
/// used going forward), but the buffer bytes are left exactly as the user
/// last typed them — this session's baseline simply anchors from the disk
/// content `db_enqueue::load_document`'s caller already read, same as
/// `recovered == disk_content` would.
pub fn handle_load_ack(
    app: &mut App,
    id: DocumentId,
    load_result: LoadResult,
    issued_version: Option<u64>,
) {
    let Some(expect_obs) = load_result.saved_obs else {
        app.set_status(
            "crash recovery unavailable for this tab: load returned no baseline observation",
            StatusSource::Other,
        );
        return;
    };

    let refusal = {
        let Some(doc) = app.doc_mut(id) else { return };
        if issued_version == Some(doc.buffer.version()) {
            match doc.hydrate(&load_result.disk_content, &load_result.recovered) {
                crate::document::Hydration::Refused(reason) => Some(reason),
                crate::document::Hydration::NoChange | crate::document::Hydration::Adopted => None,
            }
        } else {
            None
        }
    };
    if let Some(reason) = refusal {
        app.set_status(format!("crash recovery: {reason}"), StatusSource::Other);
    }
    // Dirty is a content comparison now (plan WP1) — `hydrate` no longer
    // marks it itself, so every hydration site re-derives it explicitly
    // (CONSTITUTION §1.4.8), even on the `NoChange`/version-moved-on
    // branches where `hydrate` was never actually called: this document's
    // `db` binding is about to change below, which is itself a fact worth
    // re-settling the cache against.
    crate::materialize_ack::recompute_dirty(app, id);

    let Some(doc) = app.doc_mut(id) else { return };
    doc.db = Some(DocDb::new(
        load_result.doc_id,
        expect_obs,
        false, // bind_new: `id` is already bound to a path read straight off disk
        load_result.bridge_seq.unwrap_or(0),
    ));
}

/// Records that `seq` was durably committed for `id`'s oldest still-pending
/// `AppendEdit` — called from `app::handle_db_event`'s `Msg::Db` handler on
/// `DbEvent::Ok { result: OpOutcome::Seq(seq), .. }`, after `app.db_ops` has
/// already resolved the ack's op id to `id`. `id` no longer live (an ack
/// racing a future close) is a correct, silent drop — the document it would
/// have updated is already gone.
pub fn resolve_append_ack(app: &mut App, id: DocumentId, seq: i64) {
    let Some(doc) = app.doc_mut(id) else { return };
    if let Some(doc_db) = doc.db.as_mut() {
        doc_db.resolve_append_ack(seq);
    }
}

/// The reaction to a `CreateScratch` op's ack (plan WP0/WP3, mid-session
/// half): `row_id` is a freshly minted, never-bound scratch row — `id`'s
/// document binds to it exactly like a recovered launch-time scratch draft
/// does (`rune-cli::open::adopt_scratch_doc`), `bind_new` true because a
/// scratch row has never been bound to a real file, so its NEXT save must
/// still go through the create-only path. `expect_obs` is `0`, the same
/// fabricated, never-queried `ObsId` `adopt_scratch_doc` uses — `bind_new`
/// skips the CAS-baseline lookup entirely. `id` no longer live (the draft
/// was closed while this op was still in flight) is a correct, silent
/// drop — `close_now` already sweeps `db_ops` of any entry pointing at a
/// closed document, but a race between that sweep and this ack landing is
/// still just a document that's gone, nothing to bind.
pub fn handle_create_scratch_ack(app: &mut App, id: DocumentId, row_id: i64) {
    let Some(doc) = app.doc_mut(id) else { return };
    doc.db = Some(DocDb::new(row_id, 0, true, 0));
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::db::{Db, DbBridge};
    use crate::db_enqueue::append_edit;
    use rune_core::buffer::Buffer;
    use rune_db::{ClockFn, DbEvent, OpOutcome, Store};
    use rune_vfs::{Mem, Vfs};
    use std::sync::Arc;

    fn in_memory_db() -> Db {
        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
        let clock: ClockFn = Arc::new(std::time::SystemTime::now);
        let store = Store::open_in_memory(clock, vfs, Box::new(|_evt| {})).expect("open store");
        let bridge = DbBridge::bootstrap();
        Db::new(store, bridge, false)
    }

    /// Plan WP1.S8: two documents each enqueue an `AppendEdit`; delivering
    /// their `DbEvent::Ok` acks (identified only by op id, via `app.db_ops`)
    /// must route each `Seq` result to the CORRECT document's `DocDb`, never
    /// crossing them.
    #[test]
    fn db_event_acks_route_to_the_correct_document_via_db_ops() {
        let mut app = App::new(
            Buffer::new("a"),
            None,
            Arc::new(Mem::new()),
            Some(in_memory_db()),
        );
        let id_a = app.active;
        let id_b = app.open_document(Buffer::new("b"));

        app.doc_mut(id_a).expect("doc a exists").db = Some(DocDb::new(1, 0, true, 0));
        app.doc_mut(id_b).expect("doc b exists").db = Some(DocDb::new(2, 0, true, 0));

        append_edit(&mut app, id_a, 1, &[], &[], &[]);
        append_edit(&mut app, id_b, 1, &[], &[], &[]);

        assert_eq!(app.db_ops.len(), 2);
        let op_for_a = *app
            .db_ops
            .iter()
            .find(|(_, pending)| pending.doc == id_a)
            .expect("op recorded for doc a")
            .0;
        let op_for_b = *app
            .db_ops
            .iter()
            .find(|(_, pending)| pending.doc == id_b)
            .expect("op recorded for doc b")
            .0;
        assert_ne!(op_for_a, op_for_b);

        // Simulate the acks arriving in reverse enqueue order — routing
        // must key off the op id, not arrival order.
        let doc_for_b = app.db_ops.remove(&op_for_b).expect("routes to doc b").doc;
        resolve_append_ack(&mut app, doc_for_b, 42);
        let doc_for_a = app.db_ops.remove(&op_for_a).expect("routes to doc a").doc;
        resolve_append_ack(&mut app, doc_for_a, 7);

        assert_eq!(
            app.doc(id_a)
                .expect("doc a exists")
                .db
                .as_ref()
                .expect("doc a has a DocDb")
                .last_known_seq,
            7
        );
        assert_eq!(
            app.doc(id_b)
                .expect("doc b exists")
                .db
                .as_ref()
                .expect("doc b has a DocDb")
                .last_known_seq,
            42
        );
        assert!(app.db_ops.is_empty());
    }

    #[test]
    fn handle_db_event_ok_seq_pops_db_ops_and_routes_to_the_right_document() {
        let mut app = App::new(
            Buffer::new("a"),
            None,
            Arc::new(Mem::new()),
            Some(in_memory_db()),
        );
        let id_a = app.active;
        let id_b = app.open_document(Buffer::new("b"));
        app.doc_mut(id_a).expect("doc a exists").db = Some(DocDb::new(1, 0, true, 0));
        app.doc_mut(id_b).expect("doc b exists").db = Some(DocDb::new(2, 0, true, 0));

        append_edit(&mut app, id_a, 1, &[], &[], &[]);
        let op_for_a = *app
            .db_ops
            .iter()
            .find(|(_, pending)| pending.doc == id_a)
            .expect("op recorded for doc a")
            .0;

        let mut effects = crate::runtime::Effects::default();
        crate::app::update(
            &mut app,
            crate::runtime::Msg::Db(DbEvent::Ok {
                id: op_for_a,
                result: OpOutcome::Seq(99),
            }),
            &mut effects,
        );

        assert!(
            !app.db_ops.contains_key(&op_for_a),
            "a resolved ack must be popped from db_ops"
        );
        assert_eq!(
            app.doc(id_a)
                .expect("doc a exists")
                .db
                .as_ref()
                .expect("doc a has a DocDb")
                .last_known_seq,
            99
        );
    }

    /// Review fix: a `DbEvent::Fatal` tears the whole writer thread down —
    /// every `db_ops` entry still in flight will never receive its ack, so
    /// `handle_db_event`'s `Fatal` arm must clear the map outright rather
    /// than leaving those entries as dead weight for the rest of the
    /// session.
    #[test]
    fn handle_db_event_fatal_clears_every_in_flight_db_op() {
        let mut app = App::new(
            Buffer::new("a"),
            None,
            Arc::new(Mem::new()),
            Some(in_memory_db()),
        );
        let id_a = app.active;
        let id_b = app.open_document(Buffer::new("b"));
        app.doc_mut(id_a).expect("doc a exists").db = Some(DocDb::new(1, 0, true, 0));
        app.doc_mut(id_b).expect("doc b exists").db = Some(DocDb::new(2, 0, true, 0));

        append_edit(&mut app, id_a, 1, &[], &[], &[]);
        append_edit(&mut app, id_b, 1, &[], &[], &[]);
        assert_eq!(app.db_ops.len(), 2, "test setup: two ops in flight");

        let mut effects = crate::runtime::Effects::default();
        crate::app::update(
            &mut app,
            crate::runtime::Msg::Db(DbEvent::Fatal {
                error: "writer thread died".to_string(),
            }),
            &mut effects,
        );

        assert!(
            app.db_ops.is_empty(),
            "a Fatal event must clear every in-flight db_ops entry"
        );
        assert!(
            app.db.as_ref().expect("store still present").degraded,
            "a Fatal event must still degrade the store via on_store_failure"
        );
    }
}
