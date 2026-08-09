//! Finding 2 regression (plan WP0/WP3's mid-session gap): a fresh untitled
//! draft minted by `workspace::new_untitled_document` — either directly, or
//! as `close_now`'s replacement for the last closed document — must
//! register itself as its own scratch row in the recovery store when one is
//! live, and must do nothing (no panic, no leaked `db_ops` entry) when
//! there is no store or the store is `degraded`. Shares its `Store`/`App`
//! fixtures with the rest of the `db_wiring_*` suite via `db_wiring_common`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod db_wiring_common;

use std::sync::Arc;

use rune_db::{DbEvent, OpOutcome, Store};
use rune_tui::app;
use rune_tui::db::{Db, DbBridge, DocDb};
use rune_tui::document::DocumentId;
use rune_tui::runtime::{Effects, Msg};
use rune_tui::workspace;
use rune_vfs::{Mem, Vfs};

use db_wiring_common::{app_with_store, recv_ok};

fn pending_scratch_op(app: &app::App, id: DocumentId) -> u64 {
    *app.db_ops
        .iter()
        .find(|(_, pending)| pending.doc == id && pending.mints_scratch)
        .map(|(op_id, _)| op_id)
        .expect("new_untitled_document must enqueue a CreateScratch op when a store is live")
}

/// Minting an untitled draft with a live, non-degraded store enqueues a
/// `CreateScratch` op; delivering its ack binds a real `DocDb` (`bind_new`
/// true — a scratch row has never been bound to a real file).
#[test]
fn new_untitled_document_binds_a_doc_db_once_the_create_scratch_ack_lands() {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let (mut app, bridge) = app_with_store("new-untitled-binds", vfs);

    let id = workspace::new_untitled_document(&mut app);
    assert!(
        app.doc(id).unwrap().db.is_none(),
        "db stays None until the CreateScratch ack lands"
    );

    let op_id = pending_scratch_op(&app, id);
    let result = recv_ok(&bridge, op_id);
    let row_id = match result {
        OpOutcome::RowId(row_id) => row_id,
        other => panic!("expected OpOutcome::RowId from CreateScratch, got {other:?}"),
    };

    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Db(DbEvent::Ok {
            id: op_id,
            result: OpOutcome::RowId(row_id),
        }),
        &mut effects,
    );

    let doc_db = app.doc(id).unwrap().db.as_ref().expect("db_id bound");
    assert_eq!(doc_db.db_id, row_id);
    assert!(
        doc_db.bind_new,
        "a scratch row has never been bound to a real file, so bind_new stays true"
    );
    assert!(
        !app.db_ops.contains_key(&op_id),
        "the ack must pop its own entry out of db_ops"
    );
}

/// No store at all: minting an untitled draft leaves `db: None` and enqueues
/// nothing — the draft behaves exactly as it does today, and there is
/// nothing for a stray ack to bind onto later.
#[test]
fn new_untitled_document_with_no_store_leaves_db_none_and_enqueues_nothing() {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let mut app = app::App::new_untitled(vfs, None);

    let id = workspace::new_untitled_document(&mut app);

    assert!(app.doc(id).unwrap().db.is_none());
    assert!(
        app.db_ops.is_empty(),
        "no store means create_scratch must enqueue nothing"
    );
}

/// A `degraded` store is sticky with no reopen path (`App::is_preserved`'s
/// own doc comment) — minting an untitled draft while degraded must skip
/// the enqueue exactly like the no-store case, not silently queue an op a
/// dead writer thread will never ack.
#[test]
fn new_untitled_document_with_a_degraded_store_leaves_db_none_and_enqueues_nothing() {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let clock: rune_db::ClockFn = Arc::new(std::time::SystemTime::now);
    let store =
        Store::open_in_memory(clock, Arc::clone(&vfs), Box::new(|_evt| {})).expect("open store");
    let bridge = DbBridge::bootstrap();
    let mut app = app::App::new_untitled(Arc::clone(&vfs), None);
    app.db = Some(Db::new(store, bridge, true));

    let id = workspace::new_untitled_document(&mut app);

    assert!(app.doc(id).unwrap().db.is_none());
    assert!(
        app.db_ops.is_empty(),
        "a degraded store must skip the enqueue exactly like no store at all"
    );
}

/// `close_now`'s "closing the last document mints a replacement" path (plan
/// WP0) must go through the SAME registration, not a second, forgotten copy
/// — closing the only open document with a live store still ends with the
/// fresh Untitled's own row bound once its ack lands.
#[test]
fn closing_the_only_document_registers_the_replacement_untitled_too() {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let (mut app, bridge) = app_with_store("close-replacement-registers", vfs);
    let only = app.active;

    let mut effects = Effects::default();
    let _ = workspace::close_now(&mut app, only, &mut effects);
    let replacement = app.active;
    assert_ne!(replacement, only);

    let op_id = pending_scratch_op(&app, replacement);
    let result = recv_ok(&bridge, op_id);
    let row_id = match result {
        OpOutcome::RowId(row_id) => row_id,
        other => panic!("expected OpOutcome::RowId from CreateScratch, got {other:?}"),
    };

    app::update(
        &mut app,
        Msg::Db(DbEvent::Ok {
            id: op_id,
            result: OpOutcome::RowId(row_id),
        }),
        &mut effects,
    );

    let doc_db = app
        .doc(replacement)
        .unwrap()
        .db
        .as_ref()
        .expect("replacement's own row bound");
    assert_eq!(doc_db.db_id, row_id);
}

/// A `CreateSnapshot` ack also resolves to `OpOutcome::RowId` — the router
/// must not mistake it for a `CreateScratch` ack and bind a `DocDb` onto an
/// unrelated document (`db::PendingOp::mints_scratch`'s whole reason to
/// exist).
#[test]
fn a_create_snapshot_row_id_ack_does_not_bind_a_doc_db() {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let (mut app, _bridge) = app_with_store("snapshot-row-id-not-scratch", vfs);
    let id = app.active;
    app.doc_mut(id).unwrap().db = Some(DocDb::new(1, true, 0));

    // A bare, unrouted RowId ack with no matching db_ops entry at all must
    // be a harmless no-op — the fire-and-forget shape any snapshot ack
    // whose entry was already popped elsewhere would take.
    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Db(DbEvent::Ok {
            id: 999,
            result: OpOutcome::RowId(42),
        }),
        &mut effects,
    );

    assert_eq!(
        app.doc(id).unwrap().db.as_ref().unwrap().db_id,
        1,
        "an unrelated RowId ack must never overwrite an existing DocDb"
    );
}
