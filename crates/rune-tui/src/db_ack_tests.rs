#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::db::{Db, DbBridge, LoadPurpose};
use crate::db_enqueue::append_edit;
use rune_core::buffer::Buffer;
use rune_core::undo::EditKind;
use rune_db::{ClockFn, DbEvent, OpOutcome, Store};
use rune_vfs::{Mem, Vfs, VfsTestExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn in_memory_db() -> Db {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let clock: ClockFn = Arc::new(std::time::SystemTime::now);
    let store = Store::open_in_memory(clock, vfs, Box::new(|_evt| {})).expect("open store");
    let bridge = DbBridge::bootstrap();
    Db::new(store, bridge, false)
}

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

    app.doc_mut(id_a).expect("doc a exists").replica =
        Replica::Bound(DocDb::new(1, PublishMode::CreateOnly, rune_db::Seq(0)));
    app.doc_mut(id_b).expect("doc b exists").replica =
        Replica::Bound(DocDb::new(2, PublishMode::CreateOnly, rune_db::Seq(0)));

    append_edit(&mut app, id_a, &[], &[], &[], EditKind::Other);
    append_edit(&mut app, id_b, &[], &[], &[], EditKind::Other);

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

    let doc_for_b = app.db_ops.remove(&op_for_b).expect("routes to doc b").doc;
    resolve_append_ack(&mut app, doc_for_b, rune_db::Seq(42));
    let doc_for_a = app.db_ops.remove(&op_for_a).expect("routes to doc a").doc;
    resolve_append_ack(&mut app, doc_for_a, rune_db::Seq(7));

    assert_eq!(
        app.doc(id_a)
            .expect("doc a exists")
            .doc_db()
            .expect("doc a has a DocDb")
            .last_known_seq,
        rune_db::Seq(7)
    );
    assert_eq!(
        app.doc(id_b)
            .expect("doc b exists")
            .doc_db()
            .expect("doc b has a DocDb")
            .last_known_seq,
        rune_db::Seq(42)
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
    app.doc_mut(id_a).expect("doc a exists").replica =
        Replica::Bound(DocDb::new(1, PublishMode::CreateOnly, rune_db::Seq(0)));
    app.doc_mut(id_b).expect("doc b exists").replica =
        Replica::Bound(DocDb::new(2, PublishMode::CreateOnly, rune_db::Seq(0)));

    append_edit(&mut app, id_a, &[], &[], &[], EditKind::Other);
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
            result: OpOutcome::Seq(rune_db::Seq(99)),
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
            .doc_db()
            .expect("doc a has a DocDb")
            .last_known_seq,
        rune_db::Seq(99)
    );
}

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
    app.doc_mut(id_a).expect("doc a exists").replica =
        Replica::Bound(DocDb::new(1, PublishMode::CreateOnly, rune_db::Seq(0)));
    app.doc_mut(id_b).expect("doc b exists").replica =
        Replica::Bound(DocDb::new(2, PublishMode::CreateOnly, rune_db::Seq(0)));

    append_edit(&mut app, id_a, &[], &[], &[], EditKind::Other);
    append_edit(&mut app, id_b, &[], &[], &[], EditKind::Other);
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

#[test]
fn handle_load_ack_messages_a_non_diverged_adoption() {
    let mut app = App::new(
        Buffer::new("on disk"),
        None,
        Arc::new(Mem::new()),
        Some(in_memory_db()),
    );
    let id = app.active;
    let issued_version = app.doc(id).expect("doc exists").buffer.version();

    let load_result = rune_db::LoadResult {
        doc_id: rune_db::DocId(1),
        renamed_from: None,
        disk_content: "on disk".to_string(),
        recovered: rune_db::Recovered {
            content: "recovered draft".to_string(),
            cursors: Vec::new(),
        },
        has_history: true,
        sync: rune_db::SyncState {
            kind: rune_db::SyncKind::BufferAhead,
            ancestor: None,
            ours: rune_db::Version {
                hash: BlobHash(String::new()),
                obs: None,
            },
            theirs: None,
        },
        nlink: 1,
        saved_obs: rune_db::ObsId::new(1),
        bridge_seq: None,
        resumable_merge: None,
    };

    handle_load_ack(
        &mut app,
        id,
        load_result,
        Some(issued_version),
        LoadPurpose::Recover,
    );

    assert_eq!(
        messages::newest_text(&app),
        Some("recovered unsaved changes")
    );
}

#[test]
fn binding_only_load_does_not_rehydrate() {
    let mut app = App::new(
        Buffer::new("live edits"),
        None,
        Arc::new(Mem::new()),
        Some(in_memory_db()),
    );
    let id = app.active;
    // Starts create-only on a different db_id than the ack carries —
    // the shape the lost-create-race hand-off leaves right before its
    // binding_only Load lands.
    app.doc_mut(id).expect("doc exists").replica =
        Replica::Bound(DocDb::new(3, PublishMode::CreateOnly, rune_db::Seq(0)));
    app.install_or_join_file_binding(3, None);
    let issued_version = app.doc(id).expect("doc exists").buffer.version();

    let load_result = rune_db::LoadResult {
        doc_id: rune_db::DocId(7),
        renamed_from: None,
        disk_content: "on disk".to_string(),
        recovered: rune_db::Recovered {
            content: "a stale recovery row".to_string(),
            cursors: Vec::new(),
        },
        has_history: true,
        sync: rune_db::SyncState {
            kind: rune_db::SyncKind::Clean,
            ancestor: None,
            ours: rune_db::Version {
                hash: BlobHash(String::new()),
                obs: None,
            },
            theirs: None,
        },
        nlink: 1,
        saved_obs: rune_db::ObsId::new(42),
        bridge_seq: Some(rune_db::Seq(9)),
        resumable_merge: None,
    };

    handle_load_ack(
        &mut app,
        id,
        load_result,
        Some(issued_version),
        LoadPurpose::Rebaseline {
            expect_row: Some(3),
        },
    );

    assert_eq!(
        app.doc(id).expect("doc exists").buffer.content(),
        "live edits",
        "binding_only must never adopt recovered content into the buffer"
    );
    let doc_db = app
        .doc(id)
        .expect("doc exists")
        .doc_db()
        .expect("doc.db must be rebound to the hand-off's target row");
    assert_eq!(
        doc_db.db_id, 7,
        "a binding_only ack rebinds doc.db to the ack's OWN db_id"
    );
    assert_eq!(
        doc_db.publish_mode,
        PublishMode::OverwriteExisting,
        "a binding_only ack always installs the overwrite mode"
    );
    assert_eq!(doc_db.last_known_seq, rune_db::Seq(9));
    assert_eq!(
        app.doc(id).expect("doc exists").last_sync,
        None,
        "binding_only must never touch last_sync"
    );
    assert_eq!(
        app.file_binding(7).expect("binding exists").expect_obs,
        Some(rune_db::ObsId::new(42).expect("nonzero")),
        "the shared per-file baseline must advance for the ack's OWN db_id"
    );
}

fn load_ack_for(nlink: u64) -> (App, DocumentId) {
    let mem = Mem::new();
    mem.save_atomic(Path::new("/doc.md"), b"hello")
        .expect("seed doc.md");
    mem.set_nlink(Path::new("/doc.md"), nlink)
        .expect("set nlink");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);
    let clock: ClockFn = Arc::new(std::time::SystemTime::now);
    let bridge = DbBridge::bootstrap();
    let store =
        Store::open_in_memory(clock, Arc::clone(&vfs), bridge.on_event()).expect("open store");

    store.load(Path::new("/doc.md")).expect("enqueue load");
    let load_result = match bridge.wait_for_bootstrap_event(|_| true) {
        DbEvent::Ok {
            result: OpOutcome::Load(load),
            ..
        } => *load,
        other => panic!("expected a Load ack, got {other:?}"),
    };

    let mut app = App::new(
        Buffer::new("hello"),
        Some(PathBuf::from("/doc.md")),
        vfs,
        Some(Db::new(store, bridge, false)),
    );
    let id = app.active;
    let issued_version = app.doc(id).expect("doc exists").buffer.version();
    handle_load_ack(
        &mut app,
        id,
        load_result,
        Some(issued_version),
        LoadPurpose::Recover,
    );
    (app, id)
}

#[test]
fn load_ack_warns_on_multiple_hard_links() {
    let (app, id) = load_ack_for(2);

    assert_eq!(app.doc(id).expect("doc exists").nlink, Some(2));
    assert_eq!(
        messages::newest_text(&app),
        Some(
            "this file has 2 hard links \u{2014} saving replaces it atomically, so the other links keep the old content"
        )
    );
}

#[test]
fn load_ack_stays_silent_on_a_single_hard_link() {
    let (app, id) = load_ack_for(1);

    assert_eq!(app.doc(id).expect("doc exists").nlink, Some(1));
    assert_eq!(messages::posts(&app), 0);
}

#[test]
fn rebaseline_load_advances_expect_obs() {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    vfs.save_atomic(Path::new("/doc.md"), b"hello")
        .expect("seed doc.md");
    let clock: ClockFn = Arc::new(std::time::SystemTime::now);
    let bridge = DbBridge::bootstrap();
    let store =
        Store::open_in_memory(clock, Arc::clone(&vfs), bridge.on_event()).expect("open store");

    store.load(Path::new("/doc.md")).expect("enqueue load");
    let load = match bridge.wait_for_bootstrap_event(|_| true) {
        DbEvent::Ok {
            result: OpOutcome::Load(load),
            ..
        } => *load,
        other => panic!("expected a Load ack, got {other:?}"),
    };

    let mut app = App::new(
        Buffer::new("hello"),
        Some(PathBuf::from("/doc.md")),
        Arc::clone(&vfs),
        Some(Db::new(store, Arc::clone(&bridge), false)),
    );
    let id = app.active;
    app.doc_mut(id).expect("doc exists").replica = Replica::Bound(DocDb::new(
        load.doc_id.0,
        PublishMode::OverwriteExisting,
        rune_db::Seq(0),
    ));
    app.install_or_join_file_binding(load.doc_id.0, load.saved_obs);

    // Simulates the state a lost materialize-record ack leaves behind
    // directly — reproducing the transient writer-queue failure for real
    // would make this test racy against the writer thread.
    app.file_binding_mut(load.doc_id.0)
        .expect("binding exists")
        .pending_rebaseline_hash = Some(rune_db::hash_bytes(b"hello"));

    // An external rewrite before the re-baseline load, so the fresh
    // observation asserted below is genuinely new, not incidentally
    // identical to the seed load's own.
    vfs.save_atomic(Path::new("/doc.md"), b"hello again")
        .expect("external rewrite");

    let enqueued = crate::db_enqueue::load_document_best_effort(&mut app, id, Path::new("/doc.md"));
    assert!(
        enqueued,
        "the re-baseline Load must enqueue against a live, non-degraded store"
    );

    let rebaseline_evt = bridge.wait_for_bootstrap_event(|evt| {
        matches!(
            evt,
            DbEvent::Ok {
                result: OpOutcome::Load(_),
                ..
            }
        )
    });
    let fresh_obs = match &rebaseline_evt {
        DbEvent::Ok {
            result: OpOutcome::Load(load),
            ..
        } => load.saved_obs.expect("a fresh load carries a baseline"),
        other => panic!("expected a Load ack, got {other:?}"),
    };

    let mut effects = crate::runtime::Effects::default();
    crate::app::update(
        &mut app,
        crate::runtime::Msg::Db(rebaseline_evt),
        &mut effects,
    );

    let binding = app
        .file_binding(load.doc_id.0)
        .expect("the shared binding must survive a binding_only re-baseline");
    assert_eq!(
        binding.expect_obs,
        Some(fresh_obs),
        "expect_obs must advance to the re-baseline Load's own fresh observation"
    );
    assert!(
        binding.pending_rebaseline_hash.is_none(),
        "a landed re-baseline must clear the stashed echo hash"
    );
}
