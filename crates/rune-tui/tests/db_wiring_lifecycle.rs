//! WP6 "Done when" integration tests for the rune-tui <-> rune-db wiring's
//! open/close op bookkeeping and the bootstrap-bridge handover —
//! TODO.md's §1.6 split of the original `db_wiring.rs`. The degraded-store
//! banner and restart/hydration tests live in the sibling
//! `db_wiring_degraded.rs`/`db_wiring_hydrate.rs`; all three pull shared
//! fixtures from `db_wiring_common`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod db_wiring_common;

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc;

use rune_db::{DbEvent, OpOutcome, Store};
use rune_tui::app::{self, App};
use rune_tui::db::{Db, DbBridge};
use rune_tui::runtime::{Effects, Msg};
use rune_tui::workspace;
use rune_vfs::{Mem, Vfs};

use db_wiring_common::{app_with_store, publish, temp_db_dir};

/// Plan WP6.S6: opening an Explorer path enqueues exactly one `Load` op and
/// records it in `app.db_ops`, keyed to the newly opened document — not the
/// app's pre-existing untitled draft.
#[test]
fn open_path_enqueues_exactly_one_load_op_and_records_it_in_db_ops() {
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, _rx) = app_with_store("open-path-enqueue", vfs);
    let initial_id = app.active;

    workspace::open_path(&mut app, Path::new("/doc.md"));

    let opened_id = app.active;
    assert_ne!(
        opened_id, initial_id,
        "open_path must switch to the newly opened document"
    );
    assert_eq!(
        app.db_ops.len(),
        1,
        "open_path must enqueue exactly one op (the Load)"
    );
    assert_eq!(
        app.db_ops.values().next().map(|pending| pending.doc),
        Some(opened_id),
        "the enqueued op must be routed to the opened document, not the initial draft"
    );
    assert!(
        app.doc(opened_id).unwrap().db.is_none(),
        "db stays None until the Load ack lands"
    );
}

/// Plan WP6 regression: closing a document with a `Load` op still in flight
/// must sweep its entire `PendingOp` — routing fact and issued-version fact
/// together — out of `db_ops`, not just the routing half. Before the merge
/// into one map, `workspace::close_now`'s sweep only touched the routing
/// map, leaking the issued-version entry for every document closed while a
/// load was outstanding.
#[test]
fn closing_a_document_sweeps_its_pending_load_version() {
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, _rx) = app_with_store("close-sweeps-load-version", vfs);
    let initial_id = app.active;

    workspace::open_path(&mut app, Path::new("/doc.md"));
    let opened_id = app.active;
    assert_eq!(
        app.db_ops.len(),
        1,
        "test setup: one Load op in flight for the opened document"
    );

    // Switch back to the untitled draft before closing the opened document
    // — `close_now` only reassigns `active` away from `id` when `id` is
    // currently active, which is not what this test is exercising.
    app.active = initial_id;
    let _ = workspace::close_now(&mut app, opened_id, &mut Effects::default());

    assert!(
        app.db_ops.is_empty(),
        "closing a document must sweep every fact about its in-flight ops, \
         not just the routing half of a still-pending Load"
    );
}

/// Plan WP3.S1/S4's regression test: a two-file CLI launch opens BOTH extra
/// documents (`workspace::open_path`, exactly as `rune-cli::main`'s
/// extra-positional loop does) before `DbBridge::attach` ever runs — the
/// same bridge is still in its `Bootstrap` sink for the whole window. Before
/// the fix, any `Load` ack landing in that window went to an `mpsc::Sender`
/// whose paired receiver bootstrap hydration had already dropped, and was
/// silently lost (`let _ = tx.send(evt)`): the tab kept `db: None` all
/// session. Both documents here must still end up with `db: Some(..)` once
/// `attach` finally runs and drains what accumulated.
///
/// Deterministic, no wall-clock sleep: a THIRD op (`probe` against an
/// unrelated, already-hydrated document) is enqueued strictly AFTER both
/// documents' `Load`s. The writer thread is a single ordered FIFO (`db.rs`'s
/// own module doc) that posts each op's event before starting the next, so
/// blocking on the probe's own ack (`wait_for_bootstrap_event`) is a
/// genuine rendezvous proving both earlier `Load` acks are already sitting
/// in the bridge's `Bootstrap` buffer — never a race, never a poll loop.
#[test]
fn two_file_launch_delivers_both_load_acks_once_attach_drains_the_bootstrap_buffer() {
    let mem = Mem::new();
    publish(&mem, Path::new("/marker.md"), b"marker");
    publish(&mem, Path::new("/a.md"), b"a content");
    publish(&mem, Path::new("/b.md"), b"b content");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let dir = temp_db_dir("two-file-handover");
    let db_path = dir.join("rune-v1.db");
    let bridge = DbBridge::bootstrap();
    let (store, _warning) =
        Store::open(&db_path, Arc::clone(&vfs), bridge.on_event()).expect("open store");

    // Synchronously hydrate an unrelated marker document — purely to mint a
    // valid `doc_id` the FIFO-order probe below can target; not part of the
    // two-file scenario under test.
    let marker_op = store
        .load(Path::new("/marker.md"))
        .expect("enqueue marker load");
    let marker_doc_id = match bridge.wait_for_bootstrap_event(|evt| match evt {
        DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == marker_op,
        DbEvent::Fatal { .. } => true,
    }) {
        DbEvent::Ok {
            result: OpOutcome::Load(load),
            ..
        } => load.doc_id,
        other => panic!("expected a Load ack for the marker doc, got {other:?}"),
    };

    // `App::new_untitled` mirrors the CLI's own no-positional-file shape —
    // the bridge is still `Bootstrap` here, matching `rune-cli::main`'s
    // window between `Store::open` and `runtime::run`'s `attach` call.
    let mut app = App::new_untitled(Arc::clone(&vfs));
    app.db = Some(Db::new(store, Arc::clone(&bridge), false));

    // Exactly `rune-cli::main`'s extra-positional loop: every file after
    // the first opens through `workspace::open_path`, enqueueing its own
    // `Load` — both land in the still-`Bootstrap` bridge.
    let id_a = workspace::open_path(&mut app, Path::new("/a.md")).expect("open a");
    let id_b = workspace::open_path(&mut app, Path::new("/b.md")).expect("open b");
    assert!(app.doc(id_a).unwrap().db.is_none(), "no ack has landed yet");
    assert!(app.doc(id_b).unwrap().db.is_none(), "no ack has landed yet");

    // Enqueued strictly after both Loads — see the FIFO-ordering doc
    // comment above.
    let probe_op = app
        .db
        .as_ref()
        .expect("app has a store")
        .store
        .probe(marker_doc_id)
        .expect("enqueue probe");
    let _ = bridge.wait_for_bootstrap_event(|evt| match evt {
        DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == probe_op,
        DbEvent::Fatal { .. } => true,
    });

    // The handover itself: `runtime::run`'s one call site.
    let (tx, rx) = mpsc::channel::<Msg>();
    bridge.attach(tx);

    let mut effects = Effects::default();
    for msg in rx.try_iter() {
        app::update(&mut app, msg, &mut effects);
    }

    assert!(
        app.doc(id_a).unwrap().db.is_some(),
        "doc a's Load ack, buffered before attach, must still be delivered"
    );
    assert!(
        app.doc(id_b).unwrap().db.is_some(),
        "doc b's Load ack, buffered before attach, must still be delivered"
    );
}
