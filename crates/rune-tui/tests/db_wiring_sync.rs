//! WP2 "Done when" integration tests for the sync-state plumbing (plan
//! `merge-user-s-changes-with-idempotent-octopus.md`, WP2.S1-S5): an
//! external disk edit reaches `Document::last_sync` through a `Probe` ack
//! enqueued by `workspace::switch_to`, and the footer's passive
//! `Mode::DiskChanged` hint tracks it. Follows the `db_wiring_lifecycle.rs`
//! pattern, pulling the shared fixtures from `db_wiring_common`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod db_wiring_common;

use std::path::Path;
use std::sync::Arc;

use rune_db::{DbEvent, SyncKind};
use rune_tui::app::{self, App};
use rune_tui::footer::footer_text;
use rune_tui::runtime::{Effects, Msg};
use rune_tui::workspace;
use rune_vfs::{Mem, Vfs};

use db_wiring_common::{app_with_store, publish, recv_ok};

/// Drains the single op currently recorded in `app.db_ops` for `doc`,
/// feeding its ack through `app::update` exactly as the real runtime loop
/// would when the op's `DbEvent` arrives on `Msg::Db`.
fn drain_one_op_for(
    app: &mut App,
    bridge: &rune_tui::db::DbBridge,
    doc: rune_tui::document::DocumentId,
) {
    let op_id = *app
        .db_ops
        .iter()
        .find(|(_, pending)| pending.doc == doc)
        .expect("one op recorded for this document")
        .0;
    let result = recv_ok(bridge, op_id);
    let mut effects = Effects::default();
    app::update(
        app,
        Msg::Db(DbEvent::Ok { id: op_id, result }),
        &mut effects,
    );
}

/// Overwrites `/doc.md`'s content in place, simulating an external editor —
/// `rename_excl` refuses to publish over an existing destination, so the
/// stale file is removed first (a plain, non-atomic test fixture; the
/// probe under test only ever reads the result, never races this write).
fn external_write(vfs: &dyn Vfs, bytes: &[u8]) {
    let path = Path::new("/doc.md");
    vfs.remove(path).expect("remove the stale file");
    let temp = vfs.write_durable(path, bytes).expect("write_durable");
    vfs.rename_excl(&temp, path).expect("publish");
}

/// Plan WP2 "Done when": open a document, edit the same file externally,
/// switch tabs away and back (firing the WP2.S4 probe) — the footer must
/// show the `disk changed` hint. Restoring the original disk content and
/// probing again must make the hint disappear (the probe's own auto-adopt,
/// `probe.rs`'s doc comment).
#[test]
fn external_disk_edit_surfaces_the_footer_hint_and_clears_on_restore() {
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, bridge) = app_with_store("sync-plumbing-external-edit", Arc::clone(&vfs));
    let draft_id = app.active;

    workspace::open_path(&mut app, Path::new("/doc.md"));
    let doc_id = app.active;
    assert_ne!(doc_id, draft_id);

    // Drain the `Load` ack — installs `doc.db` and seeds `last_sync` (S3).
    drain_one_op_for(&mut app, &bridge, doc_id);
    assert_eq!(
        app.doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Clean),
        "a freshly loaded, unedited document starts Clean"
    );
    assert!(
        !footer_text(&app).contains("disk changed"),
        "no hint while Clean: {:?}",
        footer_text(&app)
    );

    // External edit: the file changes on disk, the buffer does not.
    external_write(vfs.as_ref(), b"hello world");

    // Switch away and back — `workspace::switch_to`'s own WP2.S4 probe
    // enqueue only fires for the doc actually switched ONTO.
    workspace::switch_to(&mut app, draft_id);
    workspace::switch_to(&mut app, doc_id);
    drain_one_op_for(&mut app, &bridge, doc_id);

    assert_eq!(
        app.doc(doc_id).unwrap().last_sync,
        Some(SyncKind::DiskAhead),
        "disk moved, buffer didn't: DiskAhead"
    );
    assert!(
        footer_text(&app).contains("disk changed"),
        "expected the disk-changed hint, got {:?}",
        footer_text(&app)
    );

    // Restore the original content — the next probe's auto-adopt heals it
    // back to Clean, and the hint must disappear.
    external_write(vfs.as_ref(), b"hello");

    workspace::switch_to(&mut app, draft_id);
    workspace::switch_to(&mut app, doc_id);
    drain_one_op_for(&mut app, &bridge, doc_id);

    assert_eq!(
        app.doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Clean),
        "content restored: back to Clean"
    );
    assert!(
        !footer_text(&app).contains("disk changed"),
        "hint must clear once Clean again: {:?}",
        footer_text(&app)
    );
}

/// Plan WP2.S4's own regression: a document already carrying a probe in
/// flight must not get a second one stacked on top of it by a rapid
/// away-and-back switch.
#[test]
fn switch_to_skips_a_second_probe_while_one_is_already_in_flight() {
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, bridge) = app_with_store("sync-plumbing-no-double-probe", Arc::clone(&vfs));
    let draft_id = app.active;

    workspace::open_path(&mut app, Path::new("/doc.md"));
    let doc_id = app.active;
    drain_one_op_for(&mut app, &bridge, doc_id);
    assert!(app.db_ops.is_empty(), "test setup: Load ack fully drained");

    workspace::switch_to(&mut app, draft_id);
    workspace::switch_to(&mut app, doc_id);
    assert_eq!(app.db_ops.len(), 1, "test setup: one probe now in flight");

    // Switching away and back again while that probe is still outstanding
    // must not enqueue a second one.
    workspace::switch_to(&mut app, draft_id);
    workspace::switch_to(&mut app, doc_id);
    assert_eq!(
        app.db_ops.len(),
        1,
        "a probe already in flight for this document must not be duplicated"
    );
}
