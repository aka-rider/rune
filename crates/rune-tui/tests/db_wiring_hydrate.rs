//! WP5/WP6 "Done when" integration tests for the rune-tui <-> rune-db
//! wiring's hydration paths: post-restart hydration/undo (plan WP5.S4,
//! replacing the plan's interactive manual gate) and `Load`-ack adoption
//! into `Document`/`DocDb` — TODO.md's 500-line budget split of the original
//! `db_wiring.rs`. The degraded-store banner and open/close lifecycle
//! tests live in the sibling `db_wiring_degraded.rs`/
//! `db_wiring_lifecycle.rs`; all three pull shared fixtures from
//! `db_wiring_common`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod db_wiring_common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_core::buffer::{AppliedEdit, Buffer};
use rune_core::cursor::CursorSet;
use rune_core::undo::Step;
use rune_db::{DbEvent, LoadResult, OpOutcome, Store, SyncKind, SyncState, Version};
use rune_tui::app::{self, App};
use rune_tui::commands::edit;
use rune_tui::db::{DbBridge, PendingOp};
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::{Mem, Vfs};

use db_wiring_common::{
    app_with_store, db_from, doc_db_from, open_and_load, press, publish, recv_ok, temp_db_dir,
};

/// Plan WP5 "Done when" (replaces the interactive manual gate): edits
/// journaled by one session -> a NEW `Store` opened on the SAME db path
/// (a simulated restart) hydrates the recovered content, and undo reaches
/// the pre-crash anchor.
#[test]
fn restart_hydrates_content_and_undo_reaches_the_anchor() {
    let dir = temp_db_dir("restart");
    let db_path = dir.join("rune-v1.db");
    let doc_path = Path::new("/doc.md");

    let mem = Mem::new();
    publish(&mem, doc_path, b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    // Session A: types more, never saves (materializes) to disk.
    let (store_a, bridge_a, load_a) = open_and_load(&db_path, Arc::clone(&vfs), doc_path);
    assert_eq!(load_a.disk_content, "hello");
    assert_eq!(load_a.recovered, "hello");
    let db_a = db_from(store_a, bridge_a, false);
    let doc_db_a = doc_db_from(&load_a);

    let mut app_a = App::new(
        Buffer::new(load_a.recovered.clone()),
        Some(doc_path.to_path_buf()),
        Arc::clone(&vfs),
        Some(db_a),
    );
    let id_a = app_a.active;
    app_a.doc_mut(id_a).unwrap().db = Some(doc_db_a);
    let len_a = app_a.doc(id_a).unwrap().buffer.len();
    app_a.doc_mut(id_a).unwrap().cursors = CursorSet::new(len_a);
    for ch in " world".chars() {
        press(&mut app_a, ch);
    }
    assert_eq!(app_a.doc(id_a).unwrap().buffer.content(), "hello world");
    assert!(
        app_a.db_banner.is_none(),
        "session A's own store must stay healthy throughout"
    );

    // Every journaled edit must be durably committed before "restarting" —
    // `Store::shutdown` drains its writer FIFO to empty before returning
    // (deterministic; no polling needed).
    let store_a = app_a.db.take().expect("app_a has a store").store;
    store_a.shutdown();

    // Session B (simulated restart): a brand-new `Store` on the SAME path.
    // Both "sessions" here share one OS process/pid, so the real liveness
    // check (which would see this very test process as alive) can't tell
    // them apart — override it to report session A dead, the documented,
    // supported way to simulate a genuinely dead session
    // (`Store::set_liveness_check`).
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
            result: OpOutcome::Load(r),
            ..
        } => *r,
        DbEvent::Ok { result, .. } => panic!("unexpected reply to Load: {result:?}"),
        DbEvent::Err { error, .. } => panic!("load b failed: {error}"),
        DbEvent::Fatal { error } => panic!("writer b fatal during load: {error}"),
    };

    assert_eq!(
        load_b.recovered, "hello world",
        "restart must recover session A's unsaved edits"
    );
    assert_eq!(
        load_b.disk_content, "hello",
        "the on-disk file itself was never touched — session A never saved"
    );

    // The same synthetic bridge-edit reconstruction `rune-cli::main` does
    // (plan WP5.S4) — seeds the LOCAL undo journal so undo reaches the
    // anchor in one step.
    let bridge_edit = (load_b.recovered != load_b.disk_content).then(|| AppliedEdit {
        start: 0,
        end: load_b.disk_content.len(),
        deleted: load_b.disk_content.clone(),
        insert: load_b.recovered.clone(),
    });
    let db_b = db_from(store_b, bridge_b, false);
    let doc_db_b = doc_db_from(&load_b);

    let mut app_b = App::new(
        Buffer::new(load_b.recovered.clone()),
        Some(doc_path.to_path_buf()),
        Arc::clone(&vfs),
        Some(db_b),
    );
    let id_b = app_b.active;
    app_b.doc_mut(id_b).unwrap().db = Some(doc_db_b);
    if let Some(bridge_edit) = bridge_edit {
        app_b.doc_mut(id_b).unwrap().journal.push(Step {
            edits: vec![bridge_edit],
            cursors_before: Vec::new(),
            cursors_after: Vec::new(),
        });
    }

    assert_eq!(app_b.doc(id_b).unwrap().buffer.content(), "hello world");

    edit::undo(&mut app_b, id_b);
    assert_eq!(
        app_b.doc(id_b).unwrap().buffer.content(),
        "hello",
        "post-restart undo must reach the pre-crash anchor (the disk content)"
    );
}

/// The `Load` ack installs `Document::db` as `Some` once it lands.
#[test]
fn load_ack_installs_document_db_as_some() {
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, rx) = app_with_store("open-path-ack-installs-db", vfs);
    rune_tui::workspace::open_path(&mut app, Path::new("/doc.md"));
    let id = app.active;
    let op_id = *app.db_ops.keys().next().expect("one op enqueued");

    let result = recv_ok(&rx, op_id);
    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Db(DbEvent::Ok { id: op_id, result }),
        &mut effects,
    );

    assert!(
        app.doc(id).unwrap().db.is_some(),
        "a Load ack with a saved_obs baseline must install DocDb"
    );
    assert!(
        !app.db_ops.contains_key(&op_id),
        "the ack must pop its own db_ops entry"
    );
    assert_eq!(
        app.doc(id).unwrap().buffer.content(),
        "hello",
        "no divergence to recover: the buffer stays exactly what was read off disk"
    );
}

/// Data-safety guard (plan WP6.S3): an ack for a document the user kept
/// typing into during the async round trip must NEVER clobber those
/// keystrokes — the buffer bytes stay exactly as typed, even though the
/// ack's own `recovered` content would otherwise differ from what's now on
/// screen. `DocDb` is still installed: the document's own recovery journal
/// is real and should be used going forward.
#[test]
fn ack_for_a_document_edited_during_the_round_trip_leaves_the_buffer_unchanged() {
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, rx) = app_with_store("open-path-edited-in-flight", vfs);
    rune_tui::workspace::open_path(&mut app, Path::new("/doc.md"));
    let id = app.active;
    let op_id = *app.db_ops.keys().next().expect("one op enqueued");

    // The user types while the Load round trip is still in flight — this
    // bumps the buffer's version past what was recorded at enqueue time.
    let len = app.doc(id).unwrap().buffer.len();
    app.doc_mut(id).unwrap().cursors = CursorSet::new(len);
    press(&mut app, '!');
    assert_eq!(app.doc(id).unwrap().buffer.content(), "hello!");

    let result = recv_ok(&rx, op_id);
    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Db(DbEvent::Ok { id: op_id, result }),
        &mut effects,
    );

    assert_eq!(
        app.doc(id).unwrap().buffer.content(),
        "hello!",
        "the ack must never clobber a keystroke typed during the round trip"
    );
    assert!(
        app.doc(id).unwrap().db.is_some(),
        "DocDb must still be installed even when the buffer adopt is skipped"
    );
}

/// A `Load` ack whose `LoadResult` carries no `saved_obs` baseline (should
/// not occur in practice — see `LoadResult::saved_obs`'s own doc comment;
/// exercised here directly since a real `Store::load` always adopts one on
/// a first load) must install nothing and surface a status message rather
/// than binding a document to a recovery row with no CAS baseline.
#[test]
fn ack_with_no_saved_obs_leaves_db_none_and_posts_a_message() {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let mut app = App::new(
        Buffer::new("hello"),
        Some(PathBuf::from("/doc.md")),
        vfs,
        None,
    );
    let id = app.active;

    let op_id = 1u64;
    let issued_version = app.doc(id).unwrap().buffer.version();
    app.db_ops
        .insert(op_id, PendingOp::load(id, issued_version));

    let load_result = LoadResult {
        doc_id: 1,
        renamed_from: None,
        disk_content: "hello".to_string(),
        recovered: "hello".to_string(),
        has_history: false,
        sync: SyncState {
            kind: SyncKind::Clean,
            ancestor: None,
            ours: Version {
                hash: String::new(),
                obs: None,
            },
            theirs: None,
        },
        nlink: 1,
        saved_obs: None,
        bridge_seq: None,
    };

    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Db(DbEvent::Ok {
            id: op_id,
            result: OpOutcome::Load(Box::new(load_result)),
        }),
        &mut effects,
    );

    assert!(
        app.doc(id).unwrap().db.is_none(),
        "no baseline observation means no DocDb binding"
    );
    assert_eq!(app.doc(id).unwrap().buffer.content(), "hello");
    assert!(
        rune_tui::messages::newest_text(&app)
            .is_some_and(|s| s.contains("no baseline observation")),
        "a status message must explain why crash recovery wasn't bound (got {:?})",
        rune_tui::messages::newest_text(&app)
    );
}

/// Review fix (plan WP5.S2, [rune-tui A 3]): `handle_load_ack` must refuse
/// to adopt recovered content that would empty (or drastically shrink) a
/// non-empty on-disk file — the destructive-async-reset suspicion
/// check, run through the shared `Document::hydrate` chokepoint. The buffer
/// stays exactly what was on disk, and a status message explains why.
#[test]
fn ack_refuses_to_adopt_recovered_content_that_would_empty_the_disk_content() {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let disk_content = "a whole paragraph of real content that must not vanish";
    let mut app = App::new(
        Buffer::new(disk_content),
        Some(PathBuf::from("/doc.md")),
        vfs,
        None,
    );
    let id = app.active;

    let op_id = 1u64;
    let issued_version = app.doc(id).unwrap().buffer.version();
    app.db_ops
        .insert(op_id, PendingOp::load(id, issued_version));

    let load_result = LoadResult {
        doc_id: 1,
        renamed_from: None,
        disk_content: disk_content.to_string(),
        // A suspicious "recovered" empty string — the exact destructive
        // async-reset pattern that must never be adopted silently.
        recovered: String::new(),
        has_history: false,
        sync: SyncState {
            kind: SyncKind::Clean,
            ancestor: None,
            ours: Version {
                hash: String::new(),
                obs: None,
            },
            theirs: None,
        },
        nlink: 1,
        saved_obs: Some(1),
        bridge_seq: None,
    };

    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Db(DbEvent::Ok {
            id: op_id,
            result: OpOutcome::Load(Box::new(load_result)),
        }),
        &mut effects,
    );

    assert_eq!(
        app.doc(id).unwrap().buffer.content(),
        disk_content,
        "a refused hydration must leave the buffer exactly as it was on disk"
    );
    assert!(
        !app.doc(id).unwrap().is_dirty(),
        "a refused hydration must not mark the buffer dirty"
    );
    assert!(
        app.doc(id).unwrap().db.is_some(),
        "DocDb must still be installed even when the adopt is refused"
    );
    assert!(
        rune_tui::messages::newest_text(&app).is_some_and(|s| s.contains("crash recovery")),
        "a status message must explain the refusal (got {:?})",
        rune_tui::messages::newest_text(&app)
    );
}
