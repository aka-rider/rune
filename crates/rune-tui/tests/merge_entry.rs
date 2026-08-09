//! WP3 "Done when" integration tests for merge entry (plan
//! `merge-user-s-changes-with-idempotent-octopus.md`): the `MergePrep` op,
//! `^M`, working-form install, and retitle. Follows the
//! `db_wiring_lifecycle.rs`/`db_wiring_sync.rs` pattern, pulling the shared
//! fixtures from `merge_common` (review fix F9's dedupe of what used to be
//! this file's own copy).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod merge_common;

use std::path::Path;
use std::sync::Arc;

use rune_db::SyncKind;
use rune_tui::app::App;
use rune_tui::db::DbBridge;
use rune_tui::document::DocumentId;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::merge::MergeState;
use rune_tui::workspace;
use rune_vfs::{Mem, Vfs};

use merge_common::db_wiring_common::{app_with_store, publish};
use merge_common::{ctrl, drain_one_op_for, external_write, press_key, reprobe};

/// Opens `/doc.md`, drains its `Load` ack, and returns the opened
/// document's id alongside the untitled draft it switched away from (a
/// switch target for the probe-triggering away-and-back below).
fn open_and_drain(app: &mut App, bridge: &DbBridge) -> (DocumentId, DocumentId) {
    let draft_id = app.active;
    workspace::open_path(app, Path::new("/doc.md"));
    let doc_id = app.active;
    drain_one_op_for(app, bridge, doc_id);
    (doc_id, draft_id)
}

/// Plan WP3 "Done when" (a): a diverged fixture, entered via `^M`, installs
/// the working form as ONE journaled edit, retitles the tab, and undoes
/// byte-for-byte.
#[test]
fn merge_on_a_diverged_document_installs_markers_as_one_journal_step() {
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, bridge) = app_with_store("merge-entry-diverged", Arc::clone(&vfs));
    let (doc_id, draft_id) = open_and_drain(&mut app, &bridge);
    assert_eq!(app.doc(doc_id).unwrap().last_sync, Some(SyncKind::Clean));

    // Ours changes (one journaled edit) — the buffer diverges from the
    // ancestor the `Load` recorded.
    press_key(
        &mut app,
        KeyInput {
            code: KeyCode::Char('!'),
            mods: Mods::NONE,
        },
    );
    assert_eq!(app.doc(doc_id).unwrap().buffer.content(), "!hello");
    // Drain the typed edit's own `AppendEdit` ack before reprobing — left
    // undrained, it would still be sitting in `app.db_ops` alongside the
    // probe below, and `drain_one_op_for` would have no way to tell which
    // of the two entries for this document is the probe.
    drain_one_op_for(&mut app, &bridge, doc_id);
    let pre_merge_bytes = app.doc(doc_id).unwrap().buffer.content().to_string();
    let journal_pos_before_merge = app.doc(doc_id).unwrap().journal.pos();

    // Theirs changes too — both sides moved since the ancestor: Diverged.
    external_write(vfs.as_ref(), b"disk changed this");
    reprobe(&mut app, &bridge, draft_id, doc_id);
    assert_eq!(app.doc(doc_id).unwrap().last_sync, Some(SyncKind::Diverged));

    app.active = doc_id;
    press_key(&mut app, ctrl('m'));
    assert!(matches!(app.merge, MergeState::Pending { .. }));
    drain_one_op_for(&mut app, &bridge, doc_id);

    let doc = app.doc(doc_id).unwrap();
    assert!(
        doc.buffer.content().contains("<<<<<<< editor\n"),
        "buffer must contain the ours marker: {:?}",
        doc.buffer.content()
    );
    assert!(
        doc.buffer.content().contains(">>>>>>> disk\n"),
        "buffer must contain the disk marker: {:?}",
        doc.buffer.content()
    );
    assert!(
        doc.file_name().ends_with(": editor <-> disk"),
        "tab/title name must carry the merge suffix: {:?}",
        doc.file_name()
    );
    assert!(matches!(app.merge, MergeState::Active { .. }));
    assert_eq!(
        doc.journal.pos(),
        journal_pos_before_merge + 1,
        "merge entry must push exactly one new journal step"
    );

    rune_tui::commands::edit::undo(&mut app, doc_id);
    assert_eq!(
        app.doc(doc_id).unwrap().buffer.content(),
        pre_merge_bytes,
        "one undo must restore the pre-merge buffer byte-for-byte"
    );
}

/// Plan WP3 "Done when" (b): a `DiskAhead` document with a CLEAN buffer
/// takes the zero-conflict fast path — the resolver never appears, the
/// buffer ends up byte-identical to disk, and merge mode never activates.
#[test]
fn merge_on_a_disk_ahead_clean_document_installs_disk_bytes_with_no_markers() {
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, bridge) = app_with_store("merge-entry-disk-ahead", Arc::clone(&vfs));
    let (doc_id, draft_id) = open_and_drain(&mut app, &bridge);
    let journal_pos_before_merge = app.doc(doc_id).unwrap().journal.pos();

    external_write(vfs.as_ref(), b"hello world");
    reprobe(&mut app, &bridge, draft_id, doc_id);
    assert_eq!(
        app.doc(doc_id).unwrap().last_sync,
        Some(SyncKind::DiskAhead)
    );

    app.active = doc_id;
    press_key(&mut app, ctrl('m'));
    drain_one_op_for(&mut app, &bridge, doc_id);

    let doc = app.doc(doc_id).unwrap();
    assert_eq!(doc.buffer.content(), "hello world");
    assert!(!doc.buffer.content().contains("<<<<<<<"));
    assert_eq!(
        doc.journal.pos(),
        journal_pos_before_merge + 1,
        "the clean fast path is still exactly one journal step"
    );
    assert_eq!(app.merge, MergeState::Inactive);
}

/// Plan WP3 "Done when" (c): the on-disk file is not valid UTF-8 — merge
/// entry refuses outright, with feedback, and never touches the buffer.
#[test]
fn merge_refuses_when_the_disk_file_is_not_valid_utf8() {
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, bridge) = app_with_store("merge-entry-invalid-utf8", Arc::clone(&vfs));
    let (doc_id, draft_id) = open_and_drain(&mut app, &bridge);

    external_write(vfs.as_ref(), &[0xff, 0xfe, 0x00, 0x01]);
    reprobe(&mut app, &bridge, draft_id, doc_id);
    assert!(matches!(
        app.doc(doc_id).unwrap().last_sync,
        Some(SyncKind::DiskAhead) | Some(SyncKind::Diverged)
    ));

    app.active = doc_id;
    press_key(&mut app, ctrl('m'));
    drain_one_op_for(&mut app, &bridge, doc_id);

    assert_eq!(
        app.doc(doc_id).unwrap().buffer.content(),
        "hello",
        "a UTF-8 refusal must never touch the buffer"
    );
    assert!(
        rune_tui::messages::newest_text(&app)
            .unwrap_or_default()
            .contains("not valid UTF-8"),
        "expected a UTF-8 refusal status, got {:?}",
        rune_tui::messages::newest_text(&app)
    );
    assert_eq!(app.merge, MergeState::Inactive);
}

/// `^M` pressed with no divergence hinted at all must refuse immediately,
/// with feedback, and never enqueue a `MergePrep`.
#[test]
fn merge_with_no_divergence_hint_refuses_without_enqueueing() {
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, bridge) = app_with_store("merge-entry-no-divergence", Arc::clone(&vfs));
    let (doc_id, _draft_id) = open_and_drain(&mut app, &bridge);
    assert_eq!(app.doc(doc_id).unwrap().last_sync, Some(SyncKind::Clean));

    app.active = doc_id;
    press_key(&mut app, ctrl('m'));

    assert_eq!(app.merge, MergeState::Inactive);
    assert!(app.db_ops.is_empty(), "no MergePrep should be enqueued");
    assert!(
        rune_tui::messages::newest_text(&app)
            .unwrap_or_default()
            .contains("no divergence to merge")
    );
}
