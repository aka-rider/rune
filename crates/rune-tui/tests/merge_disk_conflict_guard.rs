//! WP6 "Done when" (4a-4d) integration tests for the ⌘S disk-conflict Guard
//! (plan `merge-user-s-changes-with-idempotent-octopus.md`). Split out of
//! `merge_resync_guard.rs` to keep both files under the 500-line budget;
//! driven through `rune_fuzz::Session`, pulling shared setup from
//! `merge_common`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod merge_common;

use rune_db::SyncKind;
use rune_fuzz::Session;
use rune_tui::document::DocumentId;
use rune_tui::footer;
use rune_tui::guard::GuardKind;
use rune_tui::keymap::KeyCode;
use rune_tui::merge::MergeState;

use merge_common::{bare, ch, ctrl, external_write, save_and_ack, sup};

/// Sets up a document whose disk changed since it was opened, edits the
/// buffer, and drives a real `⌘S` all the way through the three-hop
/// materialize dance to the point where `handle_materialize_ack` raises the
/// disk-conflict Guard.
fn enter_disk_conflict_guard(disk_bytes: &[u8]) -> (Session, DocumentId) {
    let mut session = Session::open("/doc.md", "hello");
    let doc_id = session.app().active;

    assert!(session.key(ch('!')).is_none());
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        "!hello"
    );
    assert!(session.deliver_db().is_none());

    external_write(session.app().vfs.as_ref(), disk_bytes);

    save_and_ack(&mut session);

    (session, doc_id)
}

/// Plan WP6 "Done when" (4a): ⌘S on an externally-changed file raises the
/// Guard.
#[test]
fn save_on_an_externally_changed_file_raises_the_disk_conflict_guard() {
    let (session, doc_id) = enter_disk_conflict_guard(b"disk changed");
    let Some(prompt) = &session.app().guard else {
        panic!("expected the disk-conflict Guard");
    };
    assert_eq!(prompt.doc, doc_id);
    assert!(matches!(prompt.kind, GuardKind::DiskConflict));
}

/// Plan WP6 "Done when" (4b): the Guard's `[M]erge` answer enters merge
/// (`MergeState::Pending`).
#[test]
fn disk_conflict_guard_merge_answer_starts_a_merge_attempt() {
    let (mut session, doc_id) = enter_disk_conflict_guard(b"disk changed");
    assert!(session.key(ch('m')).is_none());
    assert!(session.app().guard.is_none());
    assert!(matches!(
        session.app().merge,
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
    let (mut session, doc_id) = enter_disk_conflict_guard(b"disk changed");
    let before = session
        .app()
        .doc(doc_id)
        .unwrap()
        .buffer
        .content()
        .to_string();

    assert!(session.key(bare(KeyCode::Escape)).is_none());

    assert!(session.app().guard.is_none());
    assert_eq!(session.app().merge, MergeState::Inactive);
    assert_eq!(session.app().doc(doc_id).unwrap().buffer.content(), before);
    assert!(
        footer::footer_text(session.app()).contains("\u{21c4} disk changed  ^M merge"),
        "the footer must fall through to the disk-changed hint, got: {}",
        footer::footer_text(session.app())
    );
    let log = rune_tui::messages::log_text(session.app());
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

#[test]
fn disk_conflict_guard_merge_answer_that_begin_refuses_keeps_the_guard_visible() {
    let (mut session, doc_id) = enter_disk_conflict_guard(b"disk changed");
    session.app_mut().doc_mut(doc_id).unwrap().last_sync = Some(SyncKind::Clean);

    assert!(session.key(ch('m')).is_none());

    assert!(
        session.app().guard.is_some(),
        "a refused begin must leave the disk-conflict Guard visible"
    );
    assert_eq!(session.app().merge, MergeState::Inactive);
    assert_eq!(
        rune_tui::messages::newest_text(session.app()),
        Some("no divergence to merge")
    );
}

/// Once the disk-conflict Guard is dismissed with Escape, the user is
/// never stranded — `^M` (the ONLY binding for `GlobalCommand::
/// Merge`, since Ghostty steals `⌘M`) reaches merge mode through the same
/// real key pipeline the rest of this suite drives.
#[test]
fn disk_conflict_guard_escape_then_ctrl_m_starts_a_merge_attempt() {
    let (mut session, doc_id) = enter_disk_conflict_guard(b"disk changed");

    assert!(session.key(bare(KeyCode::Escape)).is_none());
    assert!(session.app().guard.is_none());

    assert!(session.key(ctrl('m')).is_none());

    assert!(matches!(
        session.app().merge,
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
    let (mut session, doc_id) = enter_disk_conflict_guard(b"disk changed once");
    let pos_before_discard = session.app().doc(doc_id).unwrap().journal.pos();

    // The disk moves AGAIN while the Guard is still up.
    external_write(session.app().vfs.as_ref(), b"disk changed twice");

    assert!(session.key(ch('d')).is_none());
    assert!(session.app().guard.is_none());
    assert!(matches!(
        session.app().merge,
        MergeState::Pending { doc, .. } if doc == doc_id
    ));
    assert!(session.deliver_db_all().is_none());

    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        "disk changed twice",
        "Discard must adopt the LATEST disk bytes, not a stale snapshot"
    );
    assert_eq!(
        session.app().doc(doc_id).unwrap().journal.pos(),
        pos_before_discard + 1,
        "Discard installs the fresh bytes as exactly one journal step"
    );
    assert_eq!(session.app().merge, MergeState::Inactive);

    assert!(session.key(sup('z')).is_none());
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        "!hello"
    );
}
