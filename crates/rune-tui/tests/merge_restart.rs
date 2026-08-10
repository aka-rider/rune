//! Crash re-entry seam tests: a dead session's still-`active` merge row
//! is re-entered on a hydrating load when the journal reconstruction still
//! byte-matches the recorded working form, abandoned when it does not, and
//! left untouched by a binding-only load.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod merge_common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_core::buffer::{AppliedEdit, Buffer};
use rune_db::{Store, SyncKind};
use rune_tui::app::App;
use rune_tui::db::{Db, DbBridge, PendingOp};
use rune_tui::keymap::KeyCode;
use rune_tui::merge::MergeState;
use rune_tui::runtime::{Effects, Msg};
use rune_tui::workspace;
use rune_vfs::{Mem, Vfs};

use merge_common::db_wiring_common::{publish, recv_ok};
use merge_common::{
    bare, ch, ctrl, drain_all_ops_for, drain_one_op_for, external_write, press_key,
};

const ANCESTOR: &[u8] = b"one\ntwo\nthree\nfour\nfive\n";
const THEIRS: &[u8] = b"one disk\ntwo\nthree\nfour\nfive disk\n";

fn temp_db_path(label: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "rune-merge-restart-{label}-{}-{seq}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir.join("rune-db.sqlite")
}

fn app_with_store_at(db_path: &Path, vfs: Arc<dyn Vfs + Send + Sync>) -> (App, Arc<DbBridge>) {
    let bridge = DbBridge::bootstrap();
    let (store, _warning) =
        Store::open(db_path, Arc::clone(&vfs), bridge.on_event()).expect("open store");
    let db = Db::new(store, Arc::clone(&bridge), false);
    let app = App::new(Buffer::new(""), None, vfs, Some(db));
    (app, bridge)
}

/// Session A: opens `/doc.md`, edits lines 1 and 5, sees an external
/// rewrite of both, and enters the resolver — two conflicts, none resolved.
fn enter_two_conflict_merge_at(
    db_path: &Path,
    vfs: &Arc<dyn Vfs + Send + Sync>,
) -> (App, Arc<DbBridge>, rune_tui::document::DocumentId) {
    let (mut app, bridge) = app_with_store_at(db_path, Arc::clone(vfs));
    let draft_id = app.active;
    workspace::open_path(&mut app, Path::new("/doc.md"));
    let doc_id = app.active;
    drain_one_op_for(&mut app, &bridge, doc_id);

    press_key(&mut app, ch('X'));
    for _ in 0..4 {
        press_key(&mut app, bare(KeyCode::Down));
    }
    press_key(&mut app, bare(KeyCode::End));
    press_key(&mut app, ch('Z'));
    assert_eq!(
        app.doc(doc_id).unwrap().buffer.content(),
        "Xone\ntwo\nthree\nfour\nfiveZ\n"
    );
    drain_all_ops_for(&mut app, &bridge, doc_id);

    external_write(app.vfs.as_ref(), THEIRS);
    workspace::switch_to(&mut app, draft_id);
    workspace::switch_to(&mut app, doc_id);
    drain_one_op_for(&mut app, &bridge, doc_id);
    assert_eq!(app.doc(doc_id).unwrap().last_sync, Some(SyncKind::Diverged));

    app.active = doc_id;
    press_key(&mut app, ctrl('m'));
    drain_all_ops_for(&mut app, &bridge, doc_id);
    assert!(
        matches!(&app.merge, MergeState::Active { blocks, .. } if blocks.len() == 2),
        "fixture must enter a two-conflict merge, got {:?}",
        app.merge
    );
    (app, bridge, doc_id)
}

fn shutdown(mut app: App) {
    app.db.take().expect("store present").store.shutdown();
}

fn restart_session(db_path: &Path, vfs: &Arc<dyn Vfs + Send + Sync>) -> (App, Arc<DbBridge>) {
    let bridge = DbBridge::bootstrap();
    let (store, _warning) =
        Store::open(db_path, Arc::clone(vfs), bridge.on_event()).expect("reopen store");
    store.set_liveness_check(Arc::new(|_pid, _started_at| false));
    let db = Db::new(store, Arc::clone(&bridge), false);
    let app = App::new(Buffer::new(""), None, Arc::clone(vfs), Some(db));
    (app, bridge)
}

#[test]
fn merge_resumes_after_restart() {
    let db_path = temp_db_path("resume");
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), ANCESTOR);
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app_a, bridge_a, doc_a) = enter_two_conflict_merge_at(&db_path, &vfs);
    press_key(&mut app_a, ch('o'));
    assert!(
        matches!(&app_a.merge, MergeState::Active { .. }),
        "one of two resolved keeps the resolver active"
    );
    drain_all_ops_for(&mut app_a, &bridge_a, doc_a);
    shutdown(app_a);

    let (mut app_b, bridge_b) = restart_session(&db_path, &vfs);
    workspace::open_path(&mut app_b, Path::new("/doc.md"));
    let doc_b = app_b.active;
    drain_one_op_for(&mut app_b, &bridge_b, doc_b);

    let MergeState::Active { blocks, doc, .. } = &app_b.merge else {
        panic!("expected the merge to resume Active, got {:?}", app_b.merge);
    };
    assert_eq!(*doc, doc_b);
    assert_eq!(blocks.len(), 2, "both blocks travel across the restart");
    assert_eq!(
        blocks.iter().filter(|b| !b.resolved).count(),
        1,
        "exactly the unresolved block survives as unresolved"
    );
    assert!(
        rune_tui::messages::log_text(&app_b).contains("merge resumed — 1 conflict(s)"),
        "expected the resume status, got {:?}",
        rune_tui::messages::log_text(&app_b)
    );
    assert!(
        !rune_tui::messages::log_text(&app_b).contains("[^M]erge to reconcile"),
        "a resumed merge must not also post the stale ^M invitation, got {:?}",
        rune_tui::messages::log_text(&app_b)
    );
    assert_eq!(
        app_b
            .doc(doc_b)
            .unwrap()
            .buffer
            .content()
            .matches("<<<<<<<")
            .count(),
        1,
        "the working form re-hydrates with the one unresolved framed block"
    );
    assert!(
        app_b
            .doc(doc_b)
            .unwrap()
            .file_name()
            .ends_with(": editor <-> disk"),
        "the resolver's title returns with the resumed merge"
    );
    drain_all_ops_for(&mut app_b, &bridge_b, doc_b);

    press_key(&mut app_b, ch('t'));
    drain_all_ops_for(&mut app_b, &bridge_b, doc_b);
    assert_eq!(
        app_b.merge,
        MergeState::Inactive,
        "resolving the survivor completes the resumed merge"
    );
    assert!(
        !app_b
            .doc(doc_b)
            .unwrap()
            .buffer
            .content()
            .contains("<<<<<<<"),
        "no marker bytes remain after completion"
    );
    shutdown(app_b);
}

#[test]
fn journaled_edit_past_the_install_abandons_the_merge_on_restart() {
    let db_path = temp_db_path("abandon");
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), ANCESTOR);
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (app_a, _bridge_a, doc_a) = enter_two_conflict_merge_at(&db_path, &vfs);
    let db_id = app_a.doc(doc_a).unwrap().doc_db().unwrap().db_id;
    app_a
        .db
        .as_ref()
        .unwrap()
        .store
        .append_edit(
            db_id,
            &[AppliedEdit {
                start: 0,
                end: 0,
                deleted: String::new(),
                insert: "STRAY ".to_string(),
            }],
            &[],
            &[],
        )
        .expect("journal a stray edit past the install");
    shutdown(app_a);

    let (mut app_b, bridge_b) = restart_session(&db_path, &vfs);
    workspace::open_path(&mut app_b, Path::new("/doc.md"));
    let doc_b = app_b.active;
    drain_one_op_for(&mut app_b, &bridge_b, doc_b);

    assert_eq!(
        app_b.merge,
        MergeState::Inactive,
        "a reconstruction that no longer matches the recorded form must not re-enter"
    );
    assert!(
        !rune_tui::messages::log_text(&app_b).contains("merge resumed"),
        "no resume message may be posted"
    );
    let content = app_b.doc(doc_b).unwrap().buffer.content().to_string();
    assert!(
        content.starts_with("STRAY ") && content.contains("<<<<<<< editor\n"),
        "the recovered draft keeps its markers as plain text: {content:?}"
    );

    // The row was flipped to abandoned: a second full load must not
    // re-offer re-entry either.
    drain_all_ops_for(&mut app_b, &bridge_b, doc_b);
    shutdown(app_b);
    let (mut app_c, bridge_c) = restart_session(&db_path, &vfs);
    workspace::open_path(&mut app_c, Path::new("/doc.md"));
    let doc_c = app_c.active;
    drain_one_op_for(&mut app_c, &bridge_c, doc_c);
    assert_eq!(app_c.merge, MergeState::Inactive);
    drain_all_ops_for(&mut app_c, &bridge_c, doc_c);
    shutdown(app_c);
}

#[test]
fn binding_only_load_installs_nothing_and_leaves_the_row_active() {
    let db_path = temp_db_path("binding-only");
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), ANCESTOR);
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (app_a, _bridge_a, _doc_a) = enter_two_conflict_merge_at(&db_path, &vfs);
    shutdown(app_a);

    let disk = String::from_utf8(vfs.read(Path::new("/doc.md")).unwrap()).unwrap();
    let bridge_b = DbBridge::bootstrap();
    let (store_b, _warning) =
        Store::open(&db_path, Arc::clone(&vfs), bridge_b.on_event()).expect("reopen store");
    store_b.set_liveness_check(Arc::new(|_pid, _started_at| false));
    let mut app_b = App::new(
        Buffer::new(&disk),
        Some(PathBuf::from("/doc.md")),
        Arc::clone(&vfs),
        Some(Db::new(store_b, Arc::clone(&bridge_b), false)),
    );
    let doc_b = app_b.active;
    let issued_version = app_b.doc(doc_b).unwrap().buffer.version();

    let op_id = app_b
        .db
        .as_ref()
        .unwrap()
        .store
        .load(Path::new("/doc.md"))
        .expect("enqueue binding-only load");
    app_b
        .db_ops
        .insert(op_id, PendingOp::load(doc_b, issued_version, true));
    let result = recv_ok(&bridge_b, op_id);
    let mut effects = Effects::default();
    rune_tui::app::update(
        &mut app_b,
        Msg::Db(rune_db::DbEvent::Ok { id: op_id, result }),
        &mut effects,
    );

    assert_eq!(
        app_b.merge,
        MergeState::Inactive,
        "a binding-only load must install no merge state"
    );
    assert!(
        !rune_tui::messages::log_text(&app_b).contains("merge resumed"),
        "no resume message may be posted for a skipped hydration"
    );
    assert_eq!(
        app_b.doc(doc_b).unwrap().buffer.content(),
        disk,
        "a binding-only load must not touch the buffer"
    );

    // The row stayed active: the next FULL load re-offers and re-enters.
    let issued_version = app_b.doc(doc_b).unwrap().buffer.version();
    let op_id = app_b
        .db
        .as_ref()
        .unwrap()
        .store
        .load(Path::new("/doc.md"))
        .expect("enqueue full load");
    app_b
        .db_ops
        .insert(op_id, PendingOp::load(doc_b, issued_version, false));
    let result = recv_ok(&bridge_b, op_id);
    let mut effects = Effects::default();
    rune_tui::app::update(
        &mut app_b,
        Msg::Db(rune_db::DbEvent::Ok { id: op_id, result }),
        &mut effects,
    );

    assert!(
        matches!(
            &app_b.merge,
            MergeState::Active { blocks, doc, .. }
                if *doc == doc_b && blocks.iter().filter(|b| !b.resolved).count() == 2
        ),
        "the full load must re-enter the still-active merge, got {:?}",
        app_b.merge
    );
    drain_all_ops_for(&mut app_b, &bridge_b, doc_b);
    shutdown(app_b);
}
