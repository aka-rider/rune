//! The writer thread restarts a document's local-position numbering at
//! every bind EXCEPT the same-row re-baseline it is asked to preserve — so
//! the app-side undo mapping is re-derived at each restarting bind, carried
//! verbatim across a preserved one, and a position the bound row cannot
//! express is journaled as a forward re-base, never mis-resolved into a
//! wrong-but-existing seq (which silently truncates or resurrects content
//! on recovery). Driven bare-`App` over file-backed stores so a restart
//! proves what recovery actually reconstructs.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod rename_common;

#[path = "db_wiring_common/mod.rs"]
mod db_wiring_common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_db::{DbEvent, OpOutcome, Store};
use rune_tui::app::App;
use rune_tui::db::{Db, DbBridge, DocDb};
use rune_tui::keymap::KeyCode;
use rune_tui::runtime::{CmdKind, Msg};
use rune_vfs::{Mem, Vfs};

use db_wiring_common::{publish, restarted_store_at, temp_db_dir};
use rename_common::{
    plain, send, sup, type_text, wait_for_load, wait_for_materialize_prep,
    wait_for_materialize_record,
};

const DOC: &str = "/root/a.md";

fn file_store_app(mem: &Arc<Mem>, db_path: &Path) -> (App, Arc<DbBridge>) {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let bridge = DbBridge::bootstrap();
    let (store, warning) = Store::open(db_path, Arc::clone(&vfs), bridge.on_event()).expect("open");
    assert!(warning.is_none(), "the open ladder must not degrade");

    store.load(Path::new(DOC)).expect("enqueue load");
    let load = match bridge.wait_for_bootstrap_event(|_| true) {
        DbEvent::Ok {
            result: OpOutcome::Load(load),
            ..
        } => *load,
        other => panic!("expected a Load ack, got {other:?}"),
    };

    let mut app = App::new(
        Buffer::new("a content"),
        Some(PathBuf::from(DOC)),
        vfs,
        Some(Db::new(store, Arc::clone(&bridge), false)),
    );
    app.active_doc_mut().set_doc_db_for_test(DocDb::new(
        load.doc_id.0,
        rune_tui::db::PublishMode::OverwriteExisting,
        rune_db::Seq(0),
    ));
    app.install_or_join_file_binding(load.doc_id.0, load.saved_obs);
    app.active_doc_mut().viewport.set_size(80, 23);
    app.sync_view();
    (app, bridge)
}

/// Delivers every ack still routed in `app.db_ops`, oldest first — the
/// same in-order delivery the live `Msg` channel gives, so a later `Load`
/// ack is processed with every earlier append already resolved.
fn drain_db_ops(app: &mut App, bridge: &Arc<DbBridge>) {
    while let Some(&op_id) = app.db_ops.keys().min() {
        let evt = bridge.wait_for_bootstrap_event(|evt| match evt {
            DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == op_id,
            DbEvent::Fatal { .. } => true,
        });
        send(app, Msg::Db(evt));
    }
}

fn save_round_trip(app: &mut App, bridge: &Arc<DbBridge>) {
    send(app, sup('s'));
    let prep_evt = wait_for_materialize_prep(bridge);
    let mut effects = send(app, Msg::Db(prep_evt));
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Save)
        .expect("the prepare ack must spawn the caller-side vfs Cmd");
    let vfs_done = cmd.run().expect("the vfs Cmd must reply");
    send(app, vfs_done);
    let record_evt = wait_for_materialize_record(bridge);
    send(app, Msg::Db(record_evt));
    drain_db_ops(app, bridge);
}

fn restart_recovered(mem: &Arc<Mem>, db_path: &Path) -> String {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let (store_b, bridge_b) = restarted_store_at(db_path, vfs);
    let op_id = store_b.load(Path::new(DOC)).expect("enqueue load");
    let load_evt = bridge_b.wait_for_bootstrap_event(|evt| match evt {
        DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == op_id,
        DbEvent::Fatal { .. } => true,
    });
    store_b.shutdown();
    match load_evt {
        DbEvent::Ok {
            result: OpOutcome::Load(load),
            ..
        } => load.recovered.content,
        other => panic!("a fresh session's load must not fail, got {other:?}"),
    }
}

/// HIGH 1: a same-row re-baseline `Load` (the committed-save
/// lost-bookkeeping path, `load_document_best_effort`) makes the writer
/// restart this row's local-position numbering. An undo afterwards must
/// never mis-resolve into a wrong-but-existing post-reload seq — that
/// silently records the undo at the wrong journal position, and recovery
/// resurrects the very text the user watched disappear.
#[test]
fn undo_after_a_same_row_rebaseline_never_resurrects_undone_text() {
    let dir = temp_db_dir("rebaseline-undo");
    let db_path = dir.join("rune-v1.db");
    let mem = Arc::new(Mem::new());
    publish(mem.as_ref(), Path::new(DOC), b"a content");
    let (mut app, bridge) = file_store_app(&mem, &db_path);
    let id = app.active;

    type_text(&mut app, "ab");
    drain_db_ops(&mut app, &bridge);
    save_round_trip(&mut app, &bridge);
    assert!(app.db_ops.is_empty(), "every pre-reload ack is resolved");

    assert!(
        rune_tui::db_enqueue::load_document_best_effort(&mut app, id, Path::new(DOC)),
        "the re-baseline load must enqueue"
    );
    let load_evt = wait_for_load(&bridge);
    send(&mut app, Msg::Db(load_evt));

    send(&mut app, plain(KeyCode::End));
    type_text(&mut app, "cd");
    assert_eq!(app.doc(id).unwrap().buffer.content(), "aba contentcd");

    rune_tui::commands::edit::undo(&mut app, id);
    rune_tui::commands::edit::undo(&mut app, id);
    let undone_to = app.doc(id).unwrap().buffer.content().to_string();
    assert_ne!(
        undone_to, "aba contentcd",
        "the undo presses must revert something"
    );
    drain_db_ops(&mut app, &bridge);
    assert!(
        !app.db.as_ref().unwrap().degraded,
        "the undo must not degrade the store"
    );
    assert_eq!(
        rune_tui::messages::posts(&app),
        0,
        "no ack in the whole sequence may surface an error, got {:?}",
        rune_tui::messages::newest_text(&app)
    );

    let buffer = app.doc(id).unwrap().buffer.content().to_string();
    app.db.take().expect("store wired").shutdown();
    assert_eq!(
        restart_recovered(&mem, &db_path),
        buffer,
        "crash recovery must reconstruct exactly the undone-to buffer"
    );
}

/// The content the buffer showed at each local journal position, indexed by
/// position — the ladder an undo run must walk back down, one rung per
/// press.
fn record_rung(app: &App, id: rune_tui::document::DocumentId, ladder: &mut Vec<String>) {
    let doc = app.doc(id).unwrap();
    let pos = doc.journal.pos();
    ladder.truncate(pos);
    ladder.push(doc.buffer.content().to_string());
}

/// A same-row re-baseline `Load` leaves the writer's local-position
/// numbering alone, so undo positions from BEFORE it still resolve exactly:
/// the run walks back rung by rung through the states the user actually
/// saw, and no press falls back on a forward re-base (a replace-all
/// `AppendEdit` that only re-anchors the mapping, losing the exact
/// correspondence the writer already holds).
#[test]
fn deep_undo_after_a_same_row_rebaseline_resolves_without_re_basing() {
    let dir = temp_db_dir("rebaseline-deep-undo");
    let db_path = dir.join("rune-v1.db");
    let mem = Arc::new(Mem::new());
    publish(mem.as_ref(), Path::new(DOC), b"a content");
    let (mut app, bridge) = file_store_app(&mem, &db_path);
    let id = app.active;

    let mut ladder = Vec::new();
    record_rung(&app, id, &mut ladder);
    for ch in ["a", "b"] {
        type_text(&mut app, ch);
        record_rung(&app, id, &mut ladder);
    }
    drain_db_ops(&mut app, &bridge);
    save_round_trip(&mut app, &bridge);

    assert!(
        rune_tui::db_enqueue::load_document_best_effort(&mut app, id, Path::new(DOC)),
        "the re-baseline load must enqueue"
    );
    let load_evt = wait_for_load(&bridge);
    send(&mut app, Msg::Db(load_evt));

    send(&mut app, plain(KeyCode::End));
    for ch in ["c", "d"] {
        type_text(&mut app, ch);
        record_rung(&app, id, &mut ladder);
    }
    drain_db_ops(&mut app, &bridge);
    assert!(
        ladder.len() > 3,
        "the run must reach positions from before the re-baseline, got {ladder:?}"
    );

    while app.doc(id).unwrap().journal.pos() > 0 {
        rune_tui::commands::edit::undo(&mut app, id);
        assert!(
            app.db_ops.values().all(|op| !op.is_append),
            "an undo across the re-baseline must resolve exactly, never journal a forward re-base"
        );
        let doc = app.doc(id).unwrap();
        assert_eq!(
            doc.buffer.content(),
            ladder[doc.journal.pos()],
            "the undo run must walk back through the states the user saw"
        );
        drain_db_ops(&mut app, &bridge);
    }
    assert_eq!(app.doc(id).unwrap().buffer.content(), ladder[0]);
    assert!(
        !app.db.as_ref().unwrap().degraded,
        "the undo run must not degrade the store"
    );
    assert_eq!(
        rune_tui::messages::posts(&app),
        0,
        "no ack in the whole sequence may surface an error, got {:?}",
        rune_tui::messages::newest_text(&app)
    );

    app.db.take().expect("store wired").shutdown();
    assert_eq!(
        restart_recovered(&mem, &db_path),
        ladder[0],
        "crash recovery must reconstruct exactly the fully undone buffer"
    );
}

/// HIGH 2: the launch-time scratch adoption (a recovered draft, or a
/// missing-path launch) binds the document and hydrates the draft. The
/// hydrate's synthetic bridge `Step` lands only in the LOCAL journal, so
/// the undo mapping — and the row's content re-base — must account for it,
/// or the first post-launch edit journals coordinates the scratch row
/// cannot replay and an undo resolves one entry off.
#[test]
fn launch_adopted_draft_keeps_edits_and_undo_recoverable() {
    let dir = temp_db_dir("launch-adopt-undo");
    let db_path = dir.join("rune-v1.db");
    let mem = Arc::new(Mem::new());
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
    let bridge = DbBridge::bootstrap();
    let (store, warning) =
        Store::open(&db_path, Arc::clone(&vfs), bridge.on_event()).expect("open");
    assert!(warning.is_none());
    store.create_scratch().expect("enqueue create_scratch");
    let row_id = match bridge.wait_for_bootstrap_event(|_| true) {
        DbEvent::Ok {
            result: OpOutcome::ScratchDocId(doc_id),
            ..
        } => doc_id.0,
        other => panic!("expected a ScratchDocId ack, got {other:?}"),
    };

    let mut app = App::new(
        Buffer::new(""),
        None,
        vfs,
        Some(Db::new(store, Arc::clone(&bridge), false)),
    );
    let id = app.active;
    app.active_doc_mut().viewport.set_size(80, 23);
    app.sync_view();

    rune_tui::db_ack::adopt_scratch_doc(&mut app, id, row_id, "recovered draft", &[]);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "recovered draft");

    send(&mut app, plain(KeyCode::End));
    type_text(&mut app, "!");
    assert_eq!(app.doc(id).unwrap().buffer.content(), "recovered draft!");

    rune_tui::commands::edit::undo(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "recovered draft");
    drain_db_ops(&mut app, &bridge);
    assert!(!app.db.as_ref().unwrap().degraded);

    app.db.take().expect("store wired").shutdown();

    let vfs_b: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
    let (store_b, bridge_b) = restarted_store_at(&db_path, vfs_b);
    let op_id = store_b
        .reconstruct_scratch(rune_db::DocId(row_id))
        .expect("enqueue reconstruct");
    let evt = bridge_b.wait_for_bootstrap_event(|evt| match evt {
        DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == op_id,
        DbEvent::Fatal { .. } => true,
    });
    store_b.shutdown();
    match evt {
        DbEvent::Ok {
            result: OpOutcome::Reconstructed(content),
            ..
        } => assert_eq!(
            content.map(|r| r.content).as_deref(),
            Some("recovered draft"),
            "the scratch row must reconstruct the undone-to buffer"
        ),
        other => panic!("scratch reconstruction must not fail, got {other:?}"),
    }
}

/// An undo INSIDE the `Binding` window: the local journal steps back while
/// the window's buffered steps keep every commit, so a verbatim replay
/// would land the durable journal on content the buffer no longer shows.
/// The install must reconcile to the BUFFER — recovery after a crash gives
/// back what the user last saw, not the pre-undo shape.
#[test]
fn undo_inside_the_binding_window_recovers_the_buffer_not_the_replayed_tail() {
    let dir = temp_db_dir("window-undo");
    let db_path = dir.join("rune-v1.db");
    let mem = Arc::new(Mem::new());
    publish(mem.as_ref(), Path::new(DOC), b"seed");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
    let bridge = DbBridge::bootstrap();
    let (store, warning) =
        Store::open(&db_path, Arc::clone(&vfs), bridge.on_event()).expect("open");
    assert!(warning.is_none());

    let mut app = App::new(
        Buffer::new("seed"),
        Some(PathBuf::from(DOC)),
        vfs,
        Some(Db::new(store, Arc::clone(&bridge), false)),
    );
    let id = app.active;
    app.active_doc_mut().viewport.set_size(80, 23);
    app.sync_view();

    assert!(rune_tui::db_enqueue::load_document(
        &mut app,
        id,
        Path::new(DOC),
        rune_tui::db_enqueue::LoadIntent::Recover,
    ));

    type_text(&mut app, "ab");
    send(&mut app, plain(KeyCode::Home));
    type_text(&mut app, "Z");
    assert_eq!(app.doc(id).unwrap().buffer.content(), "Zabseed");
    rune_tui::commands::edit::undo(&mut app, id);
    assert_eq!(app.doc(id).unwrap().buffer.content(), "abseed");

    let load_evt = wait_for_load(&bridge);
    send(&mut app, Msg::Db(load_evt));
    drain_db_ops(&mut app, &bridge);
    assert!(!app.db.as_ref().unwrap().degraded);

    app.db.take().expect("store wired").shutdown();
    assert_eq!(
        restart_recovered(&mem, &db_path),
        "abseed",
        "recovery must give back what the user last saw"
    );
}
