//! Finding 2 regression (a mid-session gap): a fresh untitled
//! draft minted by `workspace::new_untitled_document` — either directly, or
//! as `close_now`'s replacement for the last closed document — must
//! register itself as its own scratch row in the recovery store when one is
//! live, and must do nothing (no panic, no leaked `db_ops` entry) when
//! there is no store or the store is `degraded`. Driven through
//! `rune_fuzz::Session` wherever a live store is involved.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use rune_db::{DbEvent, ForgetOutcome, OpOutcome, Store};
use rune_fuzz::Session;
use rune_tui::app;
use rune_tui::db::{Db, DbBridge, PendingOp};
use rune_tui::document::DocumentId;
use rune_tui::runtime::{Effects, Msg};
use rune_tui::workspace;
use rune_vfs::{Mem, Vfs};

fn pump(app: &mut app::App, bridge: &DbBridge, op_id: u64) {
    let evt = rune_fuzz::driver::wait_for_db_op(bridge, op_id);
    let mut effects = Effects::default();
    app::update(app, Msg::Db(evt), &mut effects);
}

fn pending_scratch_op(app: &app::App, id: DocumentId) -> u64 {
    *app.db_ops
        .iter()
        .find(|(_, pending)| pending.doc == id)
        .map(|(op_id, _)| op_id)
        .expect("new_untitled_document must enqueue a CreateScratch op when a store is live")
}

/// Minting an untitled draft with a live, non-degraded store enqueues a
/// `CreateScratch` op; delivering its ack binds a real `DocDb` (create-only
/// — a scratch row has never been bound to a real file).
#[test]
fn new_untitled_document_binds_a_doc_db_once_the_create_scratch_ack_lands() {
    let mut session = Session::open("/doc.md", "");

    let id = workspace::new_untitled_document(session.app_mut());
    assert!(
        !session.app().doc(id).unwrap().is_store_bound(),
        "db stays None until the CreateScratch ack lands"
    );
    let op_id = pending_scratch_op(session.app(), id);

    assert!(session.deliver_db().is_none());

    let doc_db = session
        .app()
        .doc(id)
        .unwrap()
        .doc_db()
        .expect("db_id bound");
    assert!(
        doc_db.publish_mode.is_create_only(),
        "a scratch row has never been bound to a real file, so it stays create-only"
    );
    assert!(
        !session.app().db_ops.contains_key(&op_id),
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

    assert!(!app.doc(id).unwrap().is_store_bound());
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

    assert!(!app.doc(id).unwrap().is_store_bound());
    assert!(
        app.db_ops.is_empty(),
        "a degraded store must skip the enqueue exactly like no store at all"
    );
}

/// `close_now`'s "closing the last document mints a replacement" path
/// must go through the SAME registration, not a second, forgotten copy
/// — closing every open document with a live store still ends with the
/// fresh Untitled's own row bound once its ack lands.
#[test]
fn closing_the_only_document_registers_the_replacement_untitled_too() {
    let mut session = Session::open("/doc.md", "");
    let seed = session.app().active;
    let draft = session
        .app()
        .documents
        .iter()
        .map(|(&id, _)| id)
        .find(|&id| id != seed)
        .expect("the untitled draft is open alongside the seed");

    let mut effects = Effects::default();
    let _ = workspace::close_now(session.app_mut(), draft, &mut effects);
    let _ = workspace::close_now(session.app_mut(), seed, &mut effects);
    let replacement = session.app().active;
    assert_ne!(replacement, seed);
    pending_scratch_op(session.app(), replacement);

    assert!(session.deliver_db_all().is_none());

    session
        .app()
        .doc(replacement)
        .unwrap()
        .doc_db()
        .expect("replacement's own row bound");
}

/// A `CreateSnapshot` ack resolves to `OpOutcome::SnapshotRowId`, a distinct
/// variant from `CreateScratch`'s own `OpOutcome::ScratchDocId` — the router
/// must not mistake one for the other and bind a `DocDb` onto an unrelated
/// document.
#[test]
fn a_create_snapshot_row_id_ack_does_not_bind_a_doc_db() {
    let mut session = Session::open("/doc.md", "");
    let id = session.app().active;
    let bound_db_id = session
        .app()
        .doc(id)
        .unwrap()
        .doc_db()
        .expect("the seed document is store-bound after setup")
        .db_id;

    // A bare, unrouted SnapshotRowId ack with no matching db_ops entry at
    // all must be a harmless no-op — the fire-and-forget shape any snapshot
    // ack whose entry was already popped elsewhere would take.
    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Db(DbEvent::Ok {
            id: 999,
            result: OpOutcome::SnapshotRowId(42),
        }),
        &mut effects,
    );

    assert_eq!(
        session.app().doc(id).unwrap().doc_db().unwrap().db_id,
        bound_db_id,
        "an unrelated SnapshotRowId ack must never overwrite an existing DocDb"
    );
}

/// Closing a draft that was actually bound to a scratch row (a real
/// `CreateScratch` ack landed, and it carries journaled history) enqueues
/// `forget_scratch`; once its ack lands the row is gone for good — a fresh
/// `recoverable_scratch` sweep must never offer it back.
#[test]
fn closing_a_bound_draft_enqueues_forget_scratch_and_the_row_is_gone() {
    let mut session = Session::open("/doc.md", "seed");
    let draft = workspace::new_untitled_document(session.app_mut());
    let bridge = Arc::clone(&session.app().db.as_ref().unwrap().bridge);

    let create_op = pending_scratch_op(session.app(), draft);
    pump(session.app_mut(), &bridge, create_op);
    let db_id = session
        .app()
        .doc(draft)
        .unwrap()
        .doc_db()
        .expect("create_scratch ack bound a DocDb")
        .db_id;

    rune_tui::commands::edit::insert_char(session.app_mut(), draft, 'h');
    let append_op = pending_scratch_op(session.app(), draft);
    pump(session.app_mut(), &bridge, append_op);

    let mut effects = Effects::default();
    let _ = workspace::close_now(session.app_mut(), draft, &mut effects);
    let forget_op = pending_scratch_op(session.app(), draft);
    pump(session.app_mut(), &bridge, forget_op);

    assert!(
        !session.app().db_ops.contains_key(&forget_op),
        "the forget ack must pop its own db_ops entry"
    );
    assert!(
        !session.app().db.as_ref().unwrap().degraded,
        "a successful forget must never degrade recovery"
    );

    let store_op = session
        .app()
        .db
        .as_ref()
        .unwrap()
        .store
        .recoverable_scratch(0)
        .expect("enqueue recoverable_scratch");
    match rune_fuzz::driver::wait_for_db_op(&bridge, store_op) {
        DbEvent::Ok {
            result: OpOutcome::Ids(ids),
            ..
        } => assert!(
            !ids.contains(&db_id),
            "the forgotten draft's row must never be offered back for recovery"
        ),
        other => panic!("unexpected event for recoverable_scratch: {other:?}"),
    }
}

/// A `forget_scratch` op is `doc_scoped`, so its failure must post an error
/// message scoped to the (already-closed) document without degrading
/// recovery for every other open document — the same rule any other
/// doc-scoped op already follows, now proven for this one specifically.
#[test]
fn a_failed_forget_posts_an_error_without_degrading_recovery() {
    let mut session = Session::open("/doc.md", "seed");
    let draft = workspace::new_untitled_document(session.app_mut());
    let bridge = Arc::clone(&session.app().db.as_ref().unwrap().bridge);

    let create_op = pending_scratch_op(session.app(), draft);
    pump(session.app_mut(), &bridge, create_op);

    let mut effects = Effects::default();
    let _ = workspace::close_now(session.app_mut(), draft, &mut effects);
    let forget_op = pending_scratch_op(session.app(), draft);

    app::update(
        session.app_mut(),
        Msg::Db(DbEvent::Err {
            id: forget_op,
            error: "disk full".to_string(),
        }),
        &mut effects,
    );

    assert!(
        rune_tui::messages::newest_text(session.app()).is_some(),
        "a failed forget must be surfaced to the user"
    );
    assert!(
        !session.app().db.as_ref().unwrap().degraded,
        "a doc-scoped housekeeping failure must never degrade recovery for the whole session"
    );
}

/// A draft still in `Replica::Binding` (its `CreateScratch` ack never
/// landed) has no `db_id` to forget: closing it must enqueue nothing new,
/// it just loses its own still-pending create op like today.
#[test]
fn closing_a_binding_draft_enqueues_no_forget() {
    let mut session = Session::open("/doc.md", "seed");
    let draft = workspace::new_untitled_document(session.app_mut());
    assert!(
        !session.app().db_ops.is_empty(),
        "test setup: create_scratch must have been enqueued"
    );

    let mut effects = Effects::default();
    let _ = workspace::close_now(session.app_mut(), draft, &mut effects);

    assert!(
        !session
            .app()
            .db_ops
            .values()
            .any(|pending| pending.doc == draft),
        "a still-binding draft must not enqueue a forget op in place of its dropped create op"
    );
}

/// Closing a file-backed document is never a scratch row: no forget op is
/// ever enqueued for it, whatever its own `DocDb` binding looks like.
#[test]
fn closing_a_file_backed_document_enqueues_no_forget() {
    let mut session = Session::open("/doc.md", "seed");
    let seed = session.app().active;
    let _draft = workspace::new_untitled_document(session.app_mut());

    let mut effects = Effects::default();
    let _ = workspace::close_now(session.app_mut(), seed, &mut effects);

    assert!(
        !session
            .app()
            .db_ops
            .values()
            .any(|pending| pending.doc == seed),
        "closing a file-backed document must not enqueue a forget_scratch op"
    );
}

/// `ClaimedByLiveSession` means the row survives (another live `rune` still
/// owns it): the user is told their draft was kept, not silently dropped.
#[test]
fn a_claimed_by_live_session_ack_keeps_the_user_informed() {
    let mut session = Session::open("/doc.md", "seed");
    let id = session.app().active;
    let op_id = 4242;
    session
        .app_mut()
        .db_ops
        .insert(op_id, PendingOp::housekeeping(id));

    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Db(DbEvent::Ok {
            id: op_id,
            result: OpOutcome::Forget(ForgetOutcome::ClaimedByLiveSession),
        }),
        &mut effects,
    );

    assert_eq!(
        rune_tui::messages::newest_text(session.app()),
        Some("draft kept in recovery: another rune is using it")
    );
    assert!(!session.app().db_ops.contains_key(&op_id));
}
