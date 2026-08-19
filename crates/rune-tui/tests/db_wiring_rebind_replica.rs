//! The lost-create-race hand-off rebinds a document to the racer's own
//! `documents` row — a row whose durable journal knows nothing about the
//! buffer's typed content. Everything the editor installs from then on
//! (a merge landing's working-form install, plain typing) journals edits
//! whose coordinates assume the BUFFER, so the writer-side replica must be
//! re-based to the buffer at bind time or every later reconstruction
//! (probe, sync classification, crash recovery) replays those edits
//! against the racer's shorter content and fails with "edit out of
//! bounds". Driven through the same bare-`App` layer as `bind_new_named.rs`
//! (the hand-off is not reachable through the session driver's own setup),
//! against a FILE-backed store so a restart can prove what recovery would
//! actually reconstruct.
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
use rune_tui::db::{Db, DbBridge, DocDb, PublishMode};
use rune_tui::merge::MergeState;
use rune_tui::runtime::{CmdKind, Msg};
use rune_vfs::{Mem, Vfs, VfsTestExt};

use db_wiring_common::{restarted_store_at, temp_db_dir};
use rename_common::{
    UNPUBLISHED_BODY, ctrl, send, sup, type_text, wait_for_load, wait_for_materialize_prep,
    wait_for_materialize_record,
};

const RACE_PATH: &str = "/root/nope.md";

/// `rename_common::unsaved_named_app_with_store`, but over a FILE-backed
/// store at `db_path` so the test can shut it down and reopen it as a
/// fresh session — the only way to prove what crash recovery would
/// actually reconstruct.
fn unsaved_named_app_with_file_store(mem: &Arc<Mem>, db_path: &Path) -> (App, Arc<DbBridge>) {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let bridge = DbBridge::bootstrap();
    let (store, warning) = Store::open(db_path, Arc::clone(&vfs), bridge.on_event()).expect("open");
    assert!(warning.is_none(), "the open ladder must not degrade");

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
        Some(PathBuf::from(RACE_PATH)),
        vfs,
        Some(Db::new(store, Arc::clone(&bridge), false)),
    );
    app.active_doc_mut().set_doc_db_for_test(DocDb::new(
        row_id,
        PublishMode::CreateOnly,
        rune_db::Seq(0),
    ));
    app.install_or_join_file_binding(row_id, None);
    app.active_doc_mut().viewport.set_size(80, 23);
    app.sync_view();
    type_text(&mut app, UNPUBLISHED_BODY);
    (app, bridge)
}

/// Drives the ⌘S that loses the create race and delivers the hand-off's
/// `binding_only` `Load` ack, leaving the document rebound to the racer's
/// own row. Returns the scratch row id the document was bound to before.
fn lose_create_race_and_rebind(app: &mut App, bridge: &Arc<DbBridge>) -> i64 {
    let id = app.active;
    let old_db_id = app.doc_db_id(id).expect("bound to the scratch row");

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

    let load_evt = wait_for_load(bridge);
    send(app, Msg::Db(load_evt));

    let new_db_id = app.doc_db_id(id).expect("rebound to the racer's row");
    assert_ne!(
        new_db_id, old_db_id,
        "the hand-off must rebind to the racer's own row"
    );
    old_db_id
}

/// Defect 1: the racer created the target EMPTY, disk moves on after the
/// rebind, and the invited `^M` merge installs a working form — an edit
/// whose coordinates assume the buffer. The racer row's journal
/// reconstructs to the racer's empty content, so without a re-base every
/// later reconstruction replays that install (and the user's next edit)
/// against a shorter replica and dies with "edit out of bounds: [..)
/// len=0". A probe must stay healthy and a fresh session must recover
/// exactly the live buffer.
#[test]
fn merge_install_after_a_lost_create_race_rebind_keeps_recovery_replayable() {
    let dir = temp_db_dir("rebind-replica");
    let db_path = dir.join("rune-v1.db");
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new(RACE_PATH), b"")
        .expect("a concurrent creator wins first, with an empty file");
    let (mut app, bridge) = unsaved_named_app_with_file_store(&mem, &db_path);
    let id = app.active;

    lose_create_race_and_rebind(&mut app, &bridge);
    let db_id = app.doc_db_id(id).expect("rebound");

    mem.save_atomic(Path::new(RACE_PATH), b"racer line\n")
        .expect("the racer keeps writing after the rebind");

    send(&mut app, ctrl('m'));
    assert!(
        matches!(app.merge, MergeState::Pending { .. }),
        "^M after the hand-off's Diverged seed must start a merge attempt, got {:?}",
        app.merge
    );
    let prep_evt = bridge.wait_for_bootstrap_event(|evt| {
        matches!(
            evt,
            DbEvent::Ok {
                result: OpOutcome::MergePrep(_),
                ..
            } | DbEvent::Err { .. }
        )
    });
    send(&mut app, Msg::Db(prep_evt));
    assert!(
        matches!(app.merge, MergeState::Active { .. }),
        "the merge landing must install the working form, got {:?}",
        app.merge
    );

    type_text(&mut app, "!");
    let buffer_content = app.doc(id).unwrap().buffer.content().to_string();

    let probe_op = app
        .db
        .as_ref()
        .unwrap()
        .store
        .probe(rune_db::DocId(db_id))
        .expect("enqueue probe");
    let probe_evt = bridge.wait_for_bootstrap_event(|evt| match evt {
        DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == probe_op,
        DbEvent::Fatal { .. } => true,
    });
    let DbEvent::Ok {
        result: OpOutcome::Sync(sync),
        ..
    } = probe_evt
    else {
        panic!("the probe after a merge install must stay healthy, got {probe_evt:?}");
    };
    assert_eq!(
        sync.ours.hash.0,
        rune_db::hash_bytes(buffer_content.as_bytes()),
        "the writer-side reconstruction must equal the live buffer"
    );

    app.db.take().expect("store still wired").shutdown();
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
    let (store_b, bridge_b) = restarted_store_at(&db_path, vfs);
    let op_id = store_b.load(Path::new(RACE_PATH)).expect("enqueue load");
    let load_evt = bridge_b.wait_for_bootstrap_event(|evt| match evt {
        DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == op_id,
        DbEvent::Fatal { .. } => true,
    });
    store_b.shutdown();
    let DbEvent::Ok {
        result: OpOutcome::Load(load),
        ..
    } = load_evt
    else {
        panic!("a fresh session's load must not fail, got {load_evt:?}");
    };
    assert_eq!(
        load.recovered.content, buffer_content,
        "crash recovery must reconstruct exactly the live buffer"
    );
}

/// Defect 1, the plainer half: no merge at all — the user simply keeps
/// typing after the rebind. Those edits journal against the racer's row
/// too, so recovery must still reconstruct the buffer.
#[test]
fn typing_after_a_lost_create_race_rebind_keeps_recovery_replayable() {
    let dir = temp_db_dir("rebind-typing");
    let db_path = dir.join("rune-v1.db");
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new(RACE_PATH), b"")
        .expect("a concurrent creator wins first, with an empty file");
    let (mut app, bridge) = unsaved_named_app_with_file_store(&mem, &db_path);
    let id = app.active;

    lose_create_race_and_rebind(&mut app, &bridge);

    type_text(&mut app, " plus more typing");
    let buffer_content = app.doc(id).unwrap().buffer.content().to_string();

    app.db.take().expect("store still wired").shutdown();
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
    let (store_b, bridge_b) = restarted_store_at(&db_path, vfs);
    let op_id = store_b.load(Path::new(RACE_PATH)).expect("enqueue load");
    let load_evt = bridge_b.wait_for_bootstrap_event(|evt| match evt {
        DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == op_id,
        DbEvent::Fatal { .. } => true,
    });
    store_b.shutdown();
    let DbEvent::Ok {
        result: OpOutcome::Load(load),
        ..
    } = load_evt
    else {
        panic!("a fresh session's load must not fail, got {load_evt:?}");
    };
    assert_eq!(
        load.recovered.content, buffer_content,
        "crash recovery must reconstruct exactly the live buffer"
    );
}

/// Defect 2: the hand-off rebinds the document to the racer's row but used
/// to leave the abandoned scratch row's shared `FileBinding` standing — a
/// stale parallel source of truth no open document references any longer.
#[test]
fn rebind_prunes_the_abandoned_scratch_rows_file_binding() {
    let dir = temp_db_dir("rebind-binding-leak");
    let db_path = dir.join("rune-v1.db");
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new(RACE_PATH), b"racer bytes")
        .expect("a concurrent creator wins first");
    let (mut app, bridge) = unsaved_named_app_with_file_store(&mem, &db_path);
    let id = app.active;

    let old_db_id = lose_create_race_and_rebind(&mut app, &bridge);
    let new_db_id = app.doc_db_id(id).expect("rebound");

    assert!(
        app.file_binding(new_db_id).is_some(),
        "the racer row's own binding must be installed"
    );
    assert!(
        app.file_binding(old_db_id).is_none(),
        "the abandoned scratch row's binding must be pruned once nothing references it"
    );

    app.db.take().expect("store still wired").shutdown();
}
