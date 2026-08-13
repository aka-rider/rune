//! Data-loss regression (fix-reopen-dataloss): a dead session's own draft,
//! bridged from ITS baseline rather than disk's current content, must reach
//! the TUI as a dirty buffer with the "disk changed" footer hint —
//! exercised through the real open/`Load`-ack path of a `rune_fuzz::Session`
//! fed a genuinely `Diverged` load produced by two REAL `Store` sessions on
//! one db path, not a hand-built fixture.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod db_wiring_common;

use std::path::Path;
use std::sync::Arc;

use rune_db::SyncKind;
use rune_fuzz::Session;
use rune_tui::db::Db;
use rune_tui::footer::footer_text;
use rune_vfs::{Mem, Vfs};

use db_wiring_common::{publish, restarted_store_at, store_at, temp_db_dir};

#[test]
fn diverged_load_ack_installs_the_bridged_draft_dirty_with_the_disk_changed_hint() {
    let dir = temp_db_dir("diverged-load-ack");
    let db_path = dir.join("rune-v1.db");
    let mem = Arc::new(Mem::new());
    publish(mem.as_ref(), Path::new("/doc.md"), b"session A's content");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;

    // Session A: loads, types an unsaved prefix, never saves.
    let (store_a, bridge_a) = store_at(&db_path, Arc::clone(&vfs));
    let mut session_a = Session::open_with_db(
        "/doc.md",
        Arc::clone(&mem),
        Db::new(store_a, bridge_a, false),
    );
    assert!(session_a.type_("UNSAVED ").is_none());
    assert_eq!(session_a.snapshot().content, "UNSAVED session A's content");
    assert!(session_a.deliver_db_all().is_none());

    // Every journaled edit must be durably committed before session A
    // "dies" without saving.
    session_a
        .app_mut()
        .db
        .take()
        .expect("session A has a store")
        .shutdown();
    drop(session_a);

    // An external atomic-swap overwrite — disk moves on independently of
    // session A's own last-known baseline, minting a new inode.
    mem.save_atomic(Path::new("/doc.md"), b"disk moved on independently")
        .expect("external atomic swap");

    // Session B: a fresh `Store` on the same path, session A reported dead.
    let (store_b, bridge_b) = restarted_store_at(&db_path, Arc::clone(&vfs));
    let session_b = Session::open_with_db(
        "/doc.md",
        Arc::clone(&mem),
        Db::new(store_b, bridge_b, false),
    );

    let app = session_b.app();
    let doc = app.active_doc();
    assert_eq!(
        doc.buffer.content(),
        "UNSAVED session A's content",
        "must bridge from A's own baseline, never silently drop A's draft"
    );
    assert_eq!(doc.last_sync, Some(SyncKind::Diverged));
    assert!(
        doc.is_dirty(),
        "an adopted draft that differs from disk must be dirty"
    );
    assert!(
        footer_text(app).contains("disk changed"),
        "expected the disk-changed hint, got {:?}",
        footer_text(app)
    );
    assert!(
        rune_tui::messages::newest_text(app)
            .is_some_and(|s| s.contains("recovered unsaved changes") && s.contains("disk")),
        "G0: a bridged-and-diverged load must post an open-time banner, got {:?}",
        rune_tui::messages::newest_text(app)
    );
}

/// The control: a dead session's own draft bridged onto disk content that
/// has NOT moved (`BufferAhead`, not `Diverged`) installs the draft dirty
/// exactly like the diverged case above. It must never post the
/// disk-changed G0 banner — this is an ordinary unsaved edit, not a
/// recovered draft whose baseline disk has moved out from under — but the
/// adoption itself still silently swapped the buffer, so a plainer
/// recovery message is owed regardless.
#[test]
fn bridged_load_without_disk_divergence_posts_a_plain_recovery_message() {
    let dir = temp_db_dir("bridged-load-no-divergence");
    let db_path = dir.join("rune-v1.db");
    let mem = Arc::new(Mem::new());
    publish(mem.as_ref(), Path::new("/doc.md"), b"shared content");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;

    let (store_a, bridge_a) = store_at(&db_path, Arc::clone(&vfs));
    let mut session_a = Session::open_with_db(
        "/doc.md",
        Arc::clone(&mem),
        Db::new(store_a, bridge_a, false),
    );
    assert!(session_a.type_("UNSAVED ").is_none());
    assert!(session_a.deliver_db_all().is_none());
    session_a
        .app_mut()
        .db
        .take()
        .expect("session A has a store")
        .shutdown();
    drop(session_a);

    // Disk is deliberately left untouched — the dead session's own baseline
    // still matches it, so this is a plain bridge, never `Diverged`.

    let (store_b, bridge_b) = restarted_store_at(&db_path, Arc::clone(&vfs));
    let session_b = Session::open_with_db(
        "/doc.md",
        Arc::clone(&mem),
        Db::new(store_b, bridge_b, false),
    );

    let app = session_b.app();
    let doc = app.active_doc();
    assert_eq!(
        doc.buffer.content(),
        "UNSAVED shared content",
        "the bridged draft must still be adopted"
    );
    assert_eq!(
        doc.last_sync,
        Some(SyncKind::BufferAhead),
        "an unmoved disk with a bridged unsaved edit is BufferAhead, never Diverged"
    );
    assert!(doc.is_dirty());
    assert_eq!(
        rune_tui::messages::posts(app),
        1,
        "no divergence to report — the G0 banner must not also fire, got {:?}",
        rune_tui::messages::newest_text(app)
    );
    assert_eq!(
        rune_tui::messages::newest_text(app),
        Some("recovered unsaved changes"),
        "no divergence to report, but the silent buffer swap still owes the \
         user a plain recovery message"
    );
}
