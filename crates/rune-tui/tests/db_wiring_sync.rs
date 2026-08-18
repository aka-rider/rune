//! Integration tests for the sync-state plumbing: an
//! external disk edit reaches `Document::last_sync` through a `Probe` ack
//! enqueued by `workspace::switch_to`, and the footer's passive
//! `Mode::DiskChanged` hint tracks it. Driven through `rune_fuzz::Session`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::Path;

use rune_db::SyncKind;
use rune_fuzz::Session;
use rune_tui::app::App;
use rune_tui::document::DocumentId;
use rune_tui::footer::footer_text;
use rune_tui::workspace;
use rune_vfs::Vfs;

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

/// The session's untitled draft — the switch-away target a reprobe needs.
fn other_doc(app: &App, active: DocumentId) -> DocumentId {
    app.documents
        .iter()
        .map(|(&id, _)| id)
        .find(|&id| id != active)
        .expect("the untitled draft stays open alongside the seed")
}

/// Open a document, edit the same file externally,
/// switch tabs away and back (firing the switch-triggered probe) — the footer must
/// show the `disk changed` hint. Restoring the original disk content and
/// probing again must make the hint disappear (the probe's own auto-adopt,
/// `probe.rs`'s doc comment).
#[test]
fn external_disk_edit_surfaces_the_footer_hint_and_clears_on_restore() {
    let mut session = Session::open("/doc.md", "hello");
    let doc_id = session.app().active;
    let draft_id = other_doc(session.app(), doc_id);

    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Clean),
        "a freshly loaded, unedited document starts Clean"
    );
    assert!(
        !footer_text(session.app()).contains("disk changed"),
        "no hint while Clean: {:?}",
        footer_text(session.app())
    );

    // External edit: the file changes on disk, the buffer does not.
    external_write(session.app().vfs.as_ref(), b"hello world");

    // Switch away and back — `workspace::switch_to`'s own probe
    // enqueue only fires for the doc actually switched ONTO.
    workspace::switch_to(session.app_mut(), draft_id);
    workspace::switch_to(session.app_mut(), doc_id);
    assert!(session.deliver_db().is_none());

    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::DiskAhead),
        "disk moved, buffer didn't: DiskAhead"
    );
    assert!(
        footer_text(session.app()).contains("disk changed"),
        "expected the disk-changed hint, got {:?}",
        footer_text(session.app())
    );

    // Restore the original content — the next probe's auto-adopt heals it
    // back to Clean, and the hint must disappear.
    external_write(session.app().vfs.as_ref(), b"hello");

    workspace::switch_to(session.app_mut(), draft_id);
    workspace::switch_to(session.app_mut(), doc_id);
    assert!(session.deliver_db().is_none());

    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Clean),
        "content restored: back to Clean"
    );
    assert!(
        !footer_text(session.app()).contains("disk changed"),
        "hint must clear once Clean again: {:?}",
        footer_text(session.app())
    );
}

/// Regression: a document already carrying a probe in
/// flight must not get a second one stacked on top of it by a rapid
/// away-and-back switch.
#[test]
fn switch_to_skips_a_second_probe_while_one_is_already_in_flight() {
    let mut session = Session::open("/doc.md", "hello");
    let doc_id = session.app().active;
    let draft_id = other_doc(session.app(), doc_id);
    assert!(
        session.app().db_ops.is_empty(),
        "test setup: session setup fully drained"
    );

    workspace::switch_to(session.app_mut(), draft_id);
    workspace::switch_to(session.app_mut(), doc_id);
    assert_eq!(
        session.app().db_ops.len(),
        1,
        "test setup: one probe now in flight"
    );

    // Switching away and back again while that probe is still outstanding
    // must not enqueue a second one.
    workspace::switch_to(session.app_mut(), draft_id);
    workspace::switch_to(session.app_mut(), doc_id);
    assert_eq!(
        session.app().db_ops.len(),
        1,
        "a probe already in flight for this document must not be duplicated"
    );
}
