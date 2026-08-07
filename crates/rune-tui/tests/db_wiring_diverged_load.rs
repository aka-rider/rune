//! Data-loss regression (fix-reopen-dataloss): a dead session's own draft,
//! bridged from ITS baseline rather than disk's current content, must reach
//! the TUI as a dirty buffer with the "disk changed" footer hint —
//! exercised through the real public [`rune_tui::db_ack::handle_load_ack`]
//! entry point, fed a genuinely `Diverged` [`LoadResult`] produced by two
//! REAL `Store` sessions (the same two-session-on-one-path shape
//! `db_wiring_hydrate.rs`'s restart test already uses), not a hand-built
//! fixture.
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
use rune_db::{DbEvent, OpOutcome, Store, SyncKind};
use rune_tui::app::App;
use rune_tui::db_ack::handle_load_ack;
use rune_tui::footer::footer_text;
use rune_vfs::{Mem, Vfs};

use db_wiring_common::{db_from, doc_db_from, open_and_load, press, publish, temp_db_dir};

#[test]
fn diverged_load_ack_installs_the_bridged_draft_dirty_with_the_disk_changed_hint() {
    let dir = temp_db_dir("diverged-load-ack");
    let db_path = dir.join("rune-v1.db");
    let doc_path = Path::new("/doc.md");

    let mem = Mem::new();
    publish(&mem, doc_path, b"session A's content");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    // Session A: loads, types an unsaved prefix, never saves.
    let (store_a, bridge_a, load_a) = open_and_load(&db_path, Arc::clone(&vfs), doc_path);
    assert_eq!(load_a.recovered, "session A's content");
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
    app_a.doc_mut(id_a).unwrap().cursors = CursorSet::new(0);
    for ch in "UNSAVED ".chars() {
        press(&mut app_a, ch);
    }
    assert_eq!(
        app_a.doc(id_a).unwrap().buffer.content(),
        "UNSAVED session A's content"
    );

    // Every journaled edit must be durably committed before session A
    // "dies" without saving.
    let store_a = app_a.db.take().expect("app_a has a store").store;
    store_a.shutdown();

    // An external atomic-swap overwrite — disk moves on independently of
    // session A's own last-known baseline, minting a new inode.
    vfs.save_atomic(doc_path, b"disk moved on independently")
        .expect("external atomic swap");

    // Session B: a fresh `Store` on the same path. Both "sessions" share
    // this one OS process/pid, so the real liveness check can't tell them
    // apart — override it to report session A dead, the documented,
    // supported way to simulate a genuinely dead session.
    let bridge_b = rune_tui::db::DbBridge::bootstrap();
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

    assert_eq!(load_b.sync.kind, SyncKind::Diverged);
    assert_eq!(
        load_b.recovered, "UNSAVED session A's content",
        "must bridge from A's own baseline, never silently drop A's draft"
    );
    assert_eq!(load_b.disk_content, "disk moved on independently");

    // `main`/`db_bootstrap::bootstrap_db`'s own shape: the buffer starts as
    // exactly the disk content `load` read, THEN the ack adopts `recovered`
    // through `hydrate` — the real path `handle_load_ack` guards on
    // `issued_version` for.
    let db_b = db_from(store_b, bridge_b, false);
    let mut app_b = App::new(
        Buffer::new(load_b.disk_content.clone()),
        Some(doc_path.to_path_buf()),
        Arc::clone(&vfs),
        Some(db_b),
    );
    let id_b = app_b.active;
    let issued_version = Some(app_b.doc(id_b).unwrap().buffer.version());

    handle_load_ack(&mut app_b, id_b, load_b, issued_version, false);

    let doc_b = app_b.doc(id_b).unwrap();
    assert_eq!(
        doc_b.buffer.content(),
        "UNSAVED session A's content",
        "the bridged draft must be adopted into the buffer"
    );
    assert_eq!(doc_b.last_sync, Some(SyncKind::Diverged));
    assert!(
        app_b.doc(id_b).unwrap().is_dirty(),
        "an adopted draft that differs from disk must be dirty"
    );
    assert!(
        footer_text(&app_b).contains("disk changed"),
        "expected the disk-changed hint, got {:?}",
        footer_text(&app_b)
    );
    assert!(
        rune_tui::messages::newest_text(&app_b)
            .is_some_and(|s| s.contains("recovered unsaved changes") && s.contains("disk")),
        "G0: a bridged-and-diverged load must post an open-time banner, got {:?}",
        rune_tui::messages::newest_text(&app_b)
    );
}

/// The control: a dead session's own draft bridged onto disk content that
/// has NOT moved (`Inherited::Bridged`, not `Diverged` — same shape
/// `db_wiring_hydrate.rs`'s restart test exercises) installs the draft dirty
/// exactly like the diverged case above, but must NEVER post the G0 banner —
/// this is an ordinary unsaved edit, not a recovered draft whose baseline
/// disk has moved out from under.
#[test]
fn bridged_load_without_disk_divergence_posts_no_g0_banner() {
    let dir = temp_db_dir("bridged-load-no-divergence");
    let db_path = dir.join("rune-v1.db");
    let doc_path = Path::new("/doc.md");

    let mem = Mem::new();
    publish(&mem, doc_path, b"shared content");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (store_a, bridge_a, load_a) = open_and_load(&db_path, Arc::clone(&vfs), doc_path);
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
    app_a.doc_mut(id_a).unwrap().cursors = CursorSet::new(0);
    for ch in "UNSAVED ".chars() {
        press(&mut app_a, ch);
    }

    let store_a = app_a.db.take().expect("app_a has a store").store;
    store_a.shutdown();

    // Disk is deliberately left untouched — the dead session's own baseline
    // still matches it, so this is `Inherited::Bridged`, never `Diverged`.

    let bridge_b = rune_tui::db::DbBridge::bootstrap();
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
        load_b.sync.kind,
        SyncKind::BufferAhead,
        "test setup: an unmoved disk with a bridged unsaved edit is BufferAhead, never Diverged"
    );

    let db_b = db_from(store_b, bridge_b, false);
    let mut app_b = App::new(
        Buffer::new(load_b.disk_content.clone()),
        Some(doc_path.to_path_buf()),
        Arc::clone(&vfs),
        Some(db_b),
    );
    let id_b = app_b.active;
    let issued_version = Some(app_b.doc(id_b).unwrap().buffer.version());

    handle_load_ack(&mut app_b, id_b, load_b, issued_version, false);

    assert_eq!(
        app_b.doc(id_b).unwrap().buffer.content(),
        "UNSAVED shared content",
        "the bridged draft must still be adopted"
    );
    assert!(app_b.doc(id_b).unwrap().is_dirty());
    assert_eq!(
        rune_tui::messages::posts(&app_b),
        0,
        "no divergence to report — the G0 banner must not fire, got {:?}",
        rune_tui::messages::newest_text(&app_b)
    );
}
