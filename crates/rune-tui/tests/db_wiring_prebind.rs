//! Issue #84 regression suite: `local_seq` desync when an `AppendEdit`
//! replica is skipped pre-bind. Shares fixtures with the rest of the
//! `db_wiring_*` suite via `db_wiring_common`.
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
use rune_db::{DbEvent, OpOutcome, Store};
use rune_tui::app::{self, App};
use rune_tui::commands::edit;
use rune_tui::db::DbBridge;
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::{Mem, Vfs};

use db_wiring_common::{app_with_store, db_from, press, publish, temp_db_dir};

/// Delivers every `DbEvent::Ok`/`Err` reply currently buffered for the ops
/// in `op_ids`, feeding each through `app::update` in delivery order.
/// Returns every event actually delivered, in the order it arrived — a
/// caller that only cares whether all of them were `Ok` can check that
/// directly instead of asserting per-event like `db_wiring_common::
/// drain_one_op_for` does.
fn drain_all(app: &mut App, bridge: &DbBridge, op_ids: &[u64]) -> Vec<DbEvent> {
    op_ids
        .iter()
        .map(|&op_id| {
            let evt = bridge.wait_for_bootstrap_event(|evt| match evt {
                DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == op_id,
                DbEvent::Fatal { .. } => true,
            });
            let mut effects = Effects::default();
            app::update(app, Msg::Db(evt.clone()), &mut effects);
            evt
        })
        .collect()
}

/// The core issue #84 repro: a document opened through the store (a `Load`
/// in flight) receives several keystrokes BEFORE that `Load`'s own ack
/// lands. Every one of those pre-bind edits must still reach the durable
/// journal once the ack installs the document's `DocDb` — restoring the 1:1
/// correspondence between local journal positions and durable `events`
/// rows an unconditionally-skipped `AppendEdit` used to break — and undo
/// must still work cleanly afterward.
#[test]
fn prebind_edits_replay_at_bind() {
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, bridge) = app_with_store("prebind-replay", vfs);
    rune_tui::workspace::open_path(&mut app, Path::new("/doc.md"));
    let id = app.active;
    let load_op = *app.db_ops.keys().next().expect("one Load op enqueued");

    // Three keystrokes land while the Load round trip is still in flight —
    // nothing else may enqueue against the store meanwhile, so `db_ops`
    // must still hold only the one Load op.
    let len = app.doc(id).unwrap().buffer.len();
    app.doc_mut(id).unwrap().cursors = rune_core::cursor::CursorSet::new(len);
    for ch in "abc".chars() {
        press(&mut app, ch);
    }
    assert_eq!(app.doc(id).unwrap().buffer.content(), "helloabc");
    assert_eq!(
        app.db_ops.len(),
        1,
        "pre-bind edits must never enqueue their own AppendEdit while Binding"
    );
    assert!(
        !app.doc(id).unwrap().is_store_bound(),
        "the document is not yet Bound while its Load is still in flight"
    );

    // The Load ack lands — content on disk never diverged from what this
    // session read, so hydration is a plain NoChange; no bridge Step
    // pushed, `undo_base` stays 0.
    let evt = bridge.wait_for_bootstrap_event(|evt| match evt {
        DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == load_op,
        DbEvent::Fatal { .. } => true,
    });
    assert!(
        matches!(evt, DbEvent::Ok { .. }),
        "the Load itself must succeed: {evt:?}"
    );
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Db(evt), &mut effects);

    assert!(
        app.doc(id).unwrap().is_store_bound(),
        "the Load ack must install DocDb"
    );
    assert_eq!(app.doc(id).unwrap().buffer.content(), "helloabc");
    assert_eq!(
        app.doc(id).unwrap().journal.len(),
        3,
        "three local journal positions: one per pre-bind keystroke"
    );

    // Every buffered pre-bind edit must have replayed as a real AppendEdit
    // enqueue the moment the ack landed.
    let replayed_ops: Vec<u64> = app.db_ops.keys().copied().collect();
    assert_eq!(
        replayed_ops.len(),
        3,
        "all three pre-bind edits must replay as real AppendEdit enqueues"
    );

    let acks = drain_all(&mut app, &bridge, &replayed_ops);
    for evt in &acks {
        assert!(
            matches!(
                evt,
                DbEvent::Ok {
                    result: OpOutcome::Seq(_),
                    ..
                }
            ),
            "every replayed AppendEdit must land durably: {evt:?}"
        );
    }
    assert!(app.db_ops.is_empty(), "every replayed op must be acked");
    assert!(
        !app.db.as_ref().unwrap().degraded,
        "the store must stay healthy through the whole replay"
    );

    // Undo must resolve cleanly against the now-durable journal — the exact
    // failure mode issue #84 produced when a pre-bind edit was silently
    // dropped instead of replayed: the writer thread's own local-position
    // count would then be short by however many edits never reached it,
    // and MoveUndoPos would resolve to the wrong durable seq (or fail
    // outright once the desync ran past the end of `local_seq`).
    edit::undo(&mut app, id);
    let undo_op = *app.db_ops.keys().next().expect("undo enqueues MoveUndoPos");
    let undo_evt = bridge.wait_for_bootstrap_event(|evt| match evt {
        DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == undo_op,
        DbEvent::Fatal { .. } => true,
    });
    assert!(
        matches!(undo_evt, DbEvent::Ok { .. }),
        "undo's own MoveUndoPos must resolve without error: {undo_evt:?}"
    );
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Db(undo_evt), &mut effects);

    assert_eq!(app.doc(id).unwrap().buffer.content(), "helloab");
    assert!(
        !app.db.as_ref().unwrap().degraded,
        "an undo resolving cleanly must never degrade the store"
    );
}

/// An adopting hydration (a dead session's own unsaved draft, recovered and
/// bridged onto disk content by the store itself) pushes a synthetic bridge
/// `Step` directly onto the local journal — permanently offsetting it by
/// one position relative to the writer thread's own local-position count,
/// which never sees that bridge as an `AppendEdit`. `DocDb::undo_base`
/// exists to correct exactly this offset; undo/redo must resolve cleanly
/// all the way back through the bridge to the pre-crash disk anchor, with
/// no error and no store degrade.
#[test]
fn undo_after_adoption_resolves() {
    let dir = temp_db_dir("prebind-adoption");
    let db_path = dir.join("rune-v1.db");
    let doc_path = Path::new("/doc.md");

    let mem = Mem::new();
    publish(&mem, doc_path, b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    // Session A: types more, never saves (materializes) to disk, then
    // "crashes" (its own journal stays durable; the process just vanishes).
    let (store_a, bridge_a, load_a) =
        db_wiring_common::open_and_load(&db_path, Arc::clone(&vfs), doc_path);
    let db_a = db_from(store_a, bridge_a, false);
    let mut app_a = App::new(
        Buffer::new(load_a.recovered.clone()),
        Some(doc_path.to_path_buf()),
        Arc::clone(&vfs),
        Some(db_a),
    );
    let id_a = app_a.active;
    app_a
        .doc_mut(id_a)
        .unwrap()
        .set_doc_db_for_test(db_wiring_common::doc_db_from(&load_a));
    db_wiring_common::join_binding_from(&mut app_a, &load_a);
    let len_a = app_a.doc(id_a).unwrap().buffer.len();
    app_a.doc_mut(id_a).unwrap().cursors = rune_core::cursor::CursorSet::new(len_a);
    for ch in " world".chars() {
        press(&mut app_a, ch);
    }
    assert_eq!(app_a.doc(id_a).unwrap().buffer.content(), "hello world");
    let store_a = app_a.db.take().unwrap().store;
    store_a.shutdown();

    // Session B ("restart"): a brand-new `Store` on the SAME path, with the
    // real liveness check overridden to report session A dead (both halves
    // of this test share one OS process, so the real check would otherwise
    // see this very test process as alive) — opened through the ordinary
    // ASYNC `workspace::open_path`/ack path, exactly like a real restart,
    // so `db_ack::handle_load_ack` is what sets `undo_base` here, not test
    // scaffolding.
    let bridge_b = DbBridge::bootstrap();
    let (store_b, _warning) =
        Store::open(&db_path, Arc::clone(&vfs), bridge_b.on_event()).expect("open store b");
    store_b.set_liveness_check(Arc::new(|_pid, _started_at| false));
    let db_b = db_from(store_b, Arc::clone(&bridge_b), false);
    let mut app_b = App::new(Buffer::new(""), None, Arc::clone(&vfs), Some(db_b));

    rune_tui::workspace::open_path(&mut app_b, doc_path);
    let id_b = app_b.active;
    let load_op = *app_b.db_ops.keys().next().expect("one Load op enqueued");
    let load_evt = bridge_b.wait_for_bootstrap_event(|evt| match evt {
        DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == load_op,
        DbEvent::Fatal { .. } => true,
    });
    let mut effects = Effects::default();
    app::update(&mut app_b, Msg::Db(load_evt), &mut effects);

    assert_eq!(
        app_b.doc(id_b).unwrap().buffer.content(),
        "hello world",
        "the adopting Load must recover session A's unsaved edit"
    );
    assert!(app_b.doc(id_b).unwrap().is_store_bound());

    // One more edit after adoption, then undo back through it (removing
    // the typed edit), and once more (through the undo_base-corrected
    // bridge, back to the pre-crash disk anchor), then redo twice back to
    // where undo started — no error, no degrade, at every step.
    let len_b = app_b.doc(id_b).unwrap().buffer.len();
    app_b.doc_mut(id_b).unwrap().cursors = rune_core::cursor::CursorSet::new(len_b);
    press(&mut app_b, '!');
    assert_eq!(app_b.doc(id_b).unwrap().buffer.content(), "hello world!");

    for expected in ["hello world", "hello"] {
        edit::undo(&mut app_b, id_b);
        let op_id = *app_b
            .db_ops
            .keys()
            .next()
            .expect("undo enqueues MoveUndoPos");
        let evt = bridge_b.wait_for_bootstrap_event(|evt| match evt {
            DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == op_id,
            DbEvent::Fatal { .. } => true,
        });
        assert!(
            matches!(evt, DbEvent::Ok { .. }),
            "undo through the adoption bridge must resolve without error: {evt:?}"
        );
        let mut effects = Effects::default();
        app::update(&mut app_b, Msg::Db(evt), &mut effects);
        assert_eq!(app_b.doc(id_b).unwrap().buffer.content(), expected);
        assert!(!app_b.db.as_ref().unwrap().degraded);
    }

    for expected in ["hello world", "hello world!"] {
        edit::redo(&mut app_b, id_b);
        let op_id = *app_b
            .db_ops
            .keys()
            .next()
            .expect("redo enqueues MoveUndoPos");
        let evt = bridge_b.wait_for_bootstrap_event(|evt| match evt {
            DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == op_id,
            DbEvent::Fatal { .. } => true,
        });
        assert!(
            matches!(evt, DbEvent::Ok { .. }),
            "redo back through the adoption bridge must resolve without error: {evt:?}"
        );
        let mut effects = Effects::default();
        app::update(&mut app_b, Msg::Db(evt), &mut effects);
        assert_eq!(app_b.doc(id_b).unwrap().buffer.content(), expected);
        assert!(!app_b.db.as_ref().unwrap().degraded);
    }

    assert!(
        rune_tui::messages::newest_text(&app_b)
            .is_none_or(|m| !m.contains("undo failed") && !m.contains("redo failed")),
        "no undo/redo error may ever have been posted"
    );
}

/// A `MoveUndoPos` resolution failure is a fact about ONE document's own
/// local-position bookkeeping, never evidence the whole store can no
/// longer be trusted — it must surface as a per-document error and leave
/// `Db::degraded` false, unlike an `AppendEdit`/`Load` failure.
#[test]
fn undo_pos_error_is_doc_scoped() {
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, bridge) = app_with_store("prebind-undo-pos-doc-scoped", vfs);
    rune_tui::workspace::open_path(&mut app, Path::new("/doc.md"));
    let id = app.active;
    let load_op = *app.db_ops.keys().next().expect("one Load op enqueued");
    let load_evt = bridge.wait_for_bootstrap_event(|evt| match evt {
        DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == load_op,
        DbEvent::Fatal { .. } => true,
    });
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Db(load_evt), &mut effects);
    assert!(app.doc(id).unwrap().is_store_bound());

    // A local position no `AppendEdit` this session has ever run could
    // possibly resolve to — the writer thread's own `MoveUndoPos` handler
    // must refuse it as `Error::NotFound`.
    rune_tui::db_enqueue::move_undo_pos(&mut app, id, 999);
    let move_op = *app
        .db_ops
        .keys()
        .next()
        .expect("move_undo_pos enqueues an op");
    let move_evt = bridge.wait_for_bootstrap_event(|evt| match evt {
        DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == move_op,
        DbEvent::Fatal { .. } => true,
    });
    assert!(
        matches!(move_evt, DbEvent::Err { .. }),
        "an out-of-range local position must fail: {move_evt:?}"
    );
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Db(move_evt), &mut effects);

    assert!(
        !app.db.as_ref().unwrap().degraded,
        "a MoveUndoPos failure must never degrade the whole store"
    );
    assert!(
        rune_tui::messages::newest_text(&app).is_some_and(|m| m.contains("doc.md")),
        "the failure must surface as a per-document error naming the document"
    );
}

/// While `Detached` (here: a degraded store at open time), every edit is a
/// plain no-op for the replica — no `AppendEdit`/`Load` ever enqueues, and
/// there is no `Binding` window buffering anything to leak.
#[test]
fn detached_document_buffers_nothing() {
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let dir = temp_db_dir("prebind-detached");
    let db_path = dir.join("rune-v1.db");
    let bridge = DbBridge::bootstrap();
    let (store, _warning) =
        Store::open(&db_path, Arc::clone(&vfs), bridge.on_event()).expect("open store");
    let db = db_from(store, bridge, true); // degraded from the start
    let mut app = App::new(Buffer::new(""), None, Arc::clone(&vfs), Some(db));

    rune_tui::workspace::open_path(&mut app, Path::new("/doc.md"));
    let id = app.active;
    assert!(
        app.db_ops.is_empty(),
        "a degraded store must never enqueue a Load"
    );
    assert!(!app.doc(id).unwrap().is_store_bound());

    let len = app.doc(id).unwrap().buffer.len();
    app.doc_mut(id).unwrap().cursors = rune_core::cursor::CursorSet::new(len);
    for ch in "xyz".chars() {
        press(&mut app, ch);
    }
    assert_eq!(app.doc(id).unwrap().buffer.content(), "helloxyz");
    assert!(
        app.db_ops.is_empty(),
        "Detached must never enqueue an AppendEdit, and never buffer one to replay later"
    );
    assert!(!app.doc(id).unwrap().is_store_bound());
}
