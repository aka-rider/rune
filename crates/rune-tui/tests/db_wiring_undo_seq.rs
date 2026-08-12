//! Regression for the deferred-resolution `MoveUndoPos` rework: a typing
//! burst that leaves several `AppendEdit` acks in flight, followed by an
//! undo whose `MoveUndoPos` enqueues BEFORE those acks are drained, must
//! never desynchronize the durable journal from the live buffer — the
//! writer thread resolves the undo target itself, from ops it has already
//! executed, never from this app's own lagging `last_known_seq` estimate.
//! Shares fixtures with the rest of the `db_wiring_*` suite via
//! `db_wiring_common`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod db_wiring_common;

use std::path::Path;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_db::{DbEvent, Store};
use rune_tui::app::{self, App};
use rune_tui::commands::edit;
use rune_tui::db::DbBridge;
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::{Mem, Vfs};

use db_wiring_common::{
    db_from, doc_db_from, join_binding_from, open_and_load, publish, temp_db_dir,
};

/// Delivers exactly one buffered `DbEvent` for `op_id` into `app` — the
/// same shape `recv_ok` uses, but keeping the raw event instead of
/// unwrapping to `OpOutcome`, so a caller can assert it was `Ok` (not
/// `Err`) without losing that distinction.
fn deliver_one(app: &mut App, bridge: &DbBridge, op_id: u64) {
    let evt = bridge.wait_for_bootstrap_event(|evt| match evt {
        DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == op_id,
        DbEvent::Fatal { .. } => true,
    });
    assert!(
        matches!(evt, DbEvent::Ok { .. }),
        "op {op_id} must not fail: {evt:?}"
    );
    let mut effects = Effects::default();
    app::update(app, Msg::Db(evt), &mut effects);
}

/// Data-safety regression: an undo committed while several of this
/// session's own `AppendEdit` acks are still in flight must resolve to the
/// EXACT durable seq the writer thread already applied — never an estimate
/// this app-side lagging bookkeeping guesses at — so the durable journal
/// never truncates events the buffer itself never undid (the corruption
/// chain the pinned `save-inflight-sm` fuzz artifact reproduced).
#[test]
fn undo_committed_with_unacked_appends_in_flight_never_desyncs_the_durable_journal() {
    let dir = temp_db_dir("undo-seq");
    let db_path = dir.join("rune-v1.db");
    let doc_path = Path::new("/doc.md");

    let mem = Mem::new();
    publish(&mem, doc_path, b"");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (store, bridge, load) = open_and_load(&db_path, Arc::clone(&vfs), doc_path);
    let db = db_from(store, Arc::clone(&bridge), false);
    let doc_db = doc_db_from(&load);

    let mut app = App::new(
        Buffer::new(load.recovered.clone()),
        Some(doc_path.to_path_buf()),
        Arc::clone(&vfs),
        Some(db),
    );
    let id = app.active;
    app.doc_mut(id).unwrap().set_doc_db_for_test(doc_db);
    join_binding_from(&mut app, &load);
    let len = app.doc(id).unwrap().buffer.len();
    app.doc_mut(id).unwrap().cursors = CursorSet::new(len);

    // Three whole-word pastes: each is ONE journal push and ONE `AppendEdit`
    // enqueue (`edit_core::insert_text`'s multi-char batch never coalesces
    // durably — `journal_append::as_single_char_insert` only coalesces a
    // SINGLE-char insert), so each maps to its own distinct durable seq —
    // no ambiguity from the durable side's typing-run coalescing muddying
    // what this test is isolating.
    for word in ["alpha", "beta", "gamma"] {
        let mut effects = Effects::default();
        app::update(&mut app, Msg::Paste(word.to_string()), &mut effects);
    }
    assert_eq!(app.doc(id).unwrap().buffer.content(), "alphabetagamma");
    assert_eq!(app.db_ops.len(), 3, "three AppendEdit ops in flight");

    // Deliver ONLY the FIRST ack — "alpha" — a typing burst leaving the
    // other two ("beta", "gamma") still unacknowledged, exactly the
    // `last_known_seq`-lagging window the bug chain started from.
    let mut op_ids: Vec<u64> = app.db_ops.keys().copied().collect();
    op_ids.sort_unstable();
    deliver_one(&mut app, &bridge, op_ids[0]);
    assert_eq!(app.db_ops.len(), 2, "beta/gamma acks still in flight");

    // Undo once: removes "gamma" from the LOCAL buffer/journal. Its
    // `MoveUndoPos` enqueues while "beta"'s AND "gamma"'s own `AppendEdit`
    // acks are still undelivered — the writer thread has already EXECUTED
    // both (strict FIFO), so it can resolve this exactly; only this app's
    // OWN bookkeeping is behind.
    edit::undo(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "alphabeta");
    assert!(
        !app.db.as_ref().unwrap().degraded,
        "MoveUndoPos must not fail against unacked in-flight AppendEdits"
    );

    // One new edit after the undo — the exact "next AppendEdit sees a
    // corrupted current_seq" step in the bug chain: if `MoveUndoPos` above
    // had resolved to an underestimate, this append would truncate durable
    // events it never should have.
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Paste("delta".to_string()), &mut effects);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "alphabetadelta");

    // Drain every remaining in-flight ack — including the pre-undo "beta"/
    // "gamma" acks and the post-undo ops — asserting NONE of them is an
    // `Err` (a truncated/corrupted journal surfaces exactly there, via
    // `edit out of bounds`/`current_seq` mismatches on the writer side).
    while let Some(&op_id) = app.db_ops.keys().next() {
        deliver_one(&mut app, &bridge, op_id);
    }
    assert!(
        !app.db.as_ref().unwrap().degraded,
        "the store must still be healthy once every op has been acked"
    );

    // Force a probe (switch away and back) — the same disk-fact refresh a
    // real tab switch triggers; draining its ack must not surface an error
    // either.
    let probe_op = app
        .db
        .as_ref()
        .unwrap()
        .store
        .probe(rune_db::DocId(app.doc(id).unwrap().doc_db().unwrap().db_id))
        .expect("enqueue probe");
    let evt = bridge.wait_for_bootstrap_event(|evt| match evt {
        DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == probe_op,
        DbEvent::Fatal { .. } => true,
    });
    assert!(
        matches!(evt, DbEvent::Ok { .. }),
        "probe must not fail: {evt:?}"
    );

    let live_content = app.doc(id).unwrap().buffer.content().to_string();
    assert_eq!(live_content, "alphabetadelta");

    // Restart: a brand-new `Store` on the SAME db path must recover
    // EXACTLY this session's live buffer — the definitive proof the
    // durable journal was never truncated behind an underestimated undo
    // target (mirrors `db_wiring_hydrate.rs`'s own restart-recovery shape).
    let store = app.db.take().unwrap().store;
    store.shutdown();

    let bridge_b = DbBridge::bootstrap();
    let (store_b, _warning) =
        Store::open(&db_path, Arc::clone(&vfs), bridge_b.on_event()).expect("open store b");
    store_b.set_liveness_check(Arc::new(|_pid, _started_at| false));
    let op_id = store_b.load(doc_path).expect("enqueue load b");
    let load_b = match bridge_b.wait_for_bootstrap_event(|evt| match evt {
        DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == op_id,
        DbEvent::Fatal { .. } => true,
    }) {
        DbEvent::Ok {
            result: rune_db::OpOutcome::Load(r),
            ..
        } => *r,
        DbEvent::Ok { result, .. } => panic!("unexpected reply to Load: {result:?}"),
        DbEvent::Err { error, .. } => panic!("load b failed: {error}"),
        DbEvent::Fatal { error } => panic!("writer b fatal during load: {error}"),
    };
    store_b.shutdown();

    assert_eq!(
        load_b.recovered, live_content,
        "the recovered document must equal the live buffer — never a truncated/corrupted \
         durable journal behind an underestimated undo target"
    );
}
