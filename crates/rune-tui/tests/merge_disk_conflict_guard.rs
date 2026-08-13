//! WP6 "Done when" (4a-4d) integration tests for the ⌘S disk-conflict Guard
//! (plan `merge-user-s-changes-with-idempotent-octopus.md`). Split out of
//! `merge_resync_guard.rs` to keep both files under the 500-line budget;
//! follows the same fixture pattern, pulling shared setup from
//! `merge_common` (review fix F9's dedupe).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod merge_common;

use std::path::Path;
use std::sync::Arc;

use rune_tui::app::App;
use rune_tui::db::DbBridge;
use rune_tui::document::DocumentId;
use rune_tui::footer;
use rune_tui::guard::GuardKind;
use rune_tui::keymap::KeyCode;
use rune_tui::merge::MergeState;
use rune_tui::workspace;
use rune_vfs::{Mem, Vfs};

use merge_common::db_wiring_common::{app_with_store, publish};
use merge_common::{
    bare, ch, ctrl, drain_all_ops_for, drain_one_op_for, external_write, press_key, save_and_ack,
};

/// Sets up a document whose disk changed since it was opened, edits the
/// buffer, and drives a real `⌘S` all the way through the three-hop
/// materialize dance to the point where `handle_materialize_ack` raises the
/// disk-conflict Guard.
fn enter_disk_conflict_guard(
    disk_bytes: &[u8],
) -> (App, Arc<DbBridge>, DocumentId, Arc<dyn Vfs + Send + Sync>) {
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, bridge) = app_with_store("merge-guard", Arc::clone(&vfs));
    workspace::open_path(&mut app, Path::new("/doc.md"));
    let doc_id = app.active;
    drain_one_op_for(&mut app, &bridge, doc_id);

    press_key(&mut app, ch('!'));
    assert_eq!(app.doc(doc_id).unwrap().buffer.content(), "!hello");
    drain_one_op_for(&mut app, &bridge, doc_id);

    external_write(vfs.as_ref(), disk_bytes);

    save_and_ack(&mut app, &bridge, doc_id);

    (app, bridge, doc_id, vfs)
}

/// Plan WP6 "Done when" (4a): ⌘S on an externally-changed file raises the
/// Guard.
#[test]
fn save_on_an_externally_changed_file_raises_the_disk_conflict_guard() {
    let (app, _bridge, doc_id, _vfs) = enter_disk_conflict_guard(b"disk changed");
    let Some(prompt) = &app.guard else {
        panic!("expected the disk-conflict Guard");
    };
    assert_eq!(prompt.doc, doc_id);
    assert!(matches!(prompt.kind, GuardKind::DiskConflict));
}

/// Plan WP6 "Done when" (4b): the Guard's `[M]erge` answer enters merge
/// (`MergeState::Pending`).
#[test]
fn disk_conflict_guard_merge_answer_starts_a_merge_attempt() {
    let (mut app, _bridge, doc_id) = {
        let (app, bridge, doc_id, _vfs) = enter_disk_conflict_guard(b"disk changed");
        (app, bridge, doc_id)
    };
    press_key(&mut app, ch('m'));
    assert!(app.guard.is_none());
    assert!(matches!(
        app.merge,
        MergeState::Pending { doc, .. } if doc == doc_id
    ));
}

/// Plan WP6 "Done when" (4c): the Guard's `Esc` answer cancels touching
/// neither the buffer nor merge state. The message log never clears an
/// earlier entry, so the save-refusal message that raised this very Guard
/// stays in the log alongside the cancellation ack — but the footer itself
/// carries no memory of it: `footer::mode` ranks `Guard` above `DiskChanged`
/// only while `app.guard` is `Some`, so clearing the Guard here already
/// lets the footer fall through to the persistent disk-changed hint on its
/// own, with nothing left to strand the user behind stale text.
#[test]
fn disk_conflict_guard_escape_falls_back_to_the_disk_changed_hint() {
    let (mut app, _bridge, doc_id, _vfs) = enter_disk_conflict_guard(b"disk changed");
    let before = app.doc(doc_id).unwrap().buffer.content().to_string();

    press_key(&mut app, bare(KeyCode::Escape));

    assert!(app.guard.is_none());
    assert_eq!(app.merge, MergeState::Inactive);
    assert_eq!(app.doc(doc_id).unwrap().buffer.content(), before);
    assert!(
        footer::footer_text(&app).contains("\u{21c4} disk changed  ^M merge"),
        "the footer must fall through to the disk-changed hint, got: {}",
        footer::footer_text(&app)
    );
    let log = rune_tui::messages::log_text(&app);
    let refused_at = log
        .find("save refused")
        .expect("save refusal must be logged");
    let cancelled_at = log
        .find("save cancelled")
        .expect("the cancellation ack must be logged");
    assert!(
        refused_at < cancelled_at,
        "the save refusal must precede the cancellation ack in the log, got {log:?}"
    );
}

/// Once the disk-conflict Guard is dismissed with Escape, the user is
/// never stranded — `^M` (the ONLY binding for `GlobalCommand::
/// Merge`, since Ghostty steals `⌘M`) reaches merge mode through the same
/// real key pipeline the rest of this suite drives.
#[test]
fn disk_conflict_guard_escape_then_ctrl_m_starts_a_merge_attempt() {
    let (mut app, _bridge, doc_id, _vfs) = enter_disk_conflict_guard(b"disk changed");

    press_key(&mut app, bare(KeyCode::Escape));
    assert!(app.guard.is_none());

    press_key(&mut app, ctrl('m'));

    assert!(matches!(
        app.merge,
        MergeState::Pending { doc, .. } | MergeState::Active { doc, .. } if doc == doc_id
    ));
}

/// Plan WP6 "Done when" (4d): the Guard's `[D]iscard` answer makes the
/// buffer equal the FRESH disk bytes in one undoable step, even when the
/// disk changed AGAIN between the conflict's detection and pressing `D`
/// (plan Assumption A2 — Discard shares `Merge`'s fresh `MergePrep`
/// pipeline rather than replaying the bytes seen at guard-raise time).
#[test]
fn disk_conflict_guard_discard_adopts_the_latest_disk_bytes_in_one_step() {
    let (mut app, bridge, doc_id, vfs) = enter_disk_conflict_guard(b"disk changed once");
    let pos_before_discard = app.doc(doc_id).unwrap().journal.pos();

    // The disk moves AGAIN while the Guard is still up.
    external_write(vfs.as_ref(), b"disk changed twice");

    press_key(&mut app, ch('d'));
    assert!(app.guard.is_none());
    assert!(matches!(
        app.merge,
        MergeState::Pending { doc, .. } if doc == doc_id
    ));
    drain_all_ops_for(&mut app, &bridge, doc_id);

    assert_eq!(
        app.doc(doc_id).unwrap().buffer.content(),
        "disk changed twice",
        "Discard must adopt the LATEST disk bytes, not a stale snapshot"
    );
    assert_eq!(
        app.doc(doc_id).unwrap().journal.pos(),
        pos_before_discard + 1,
        "Discard installs the fresh bytes as exactly one journal step"
    );
    assert_eq!(app.merge, MergeState::Inactive);

    rune_tui::commands::edit::undo(&mut app, doc_id);
    assert_eq!(app.doc(doc_id).unwrap().buffer.content(), "!hello");
}
