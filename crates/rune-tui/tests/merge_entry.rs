//! WP3 "Done when" integration tests for merge entry (plan
//! `merge-user-s-changes-with-idempotent-octopus.md`): the `MergePrep` op,
//! `^M`, working-form install, and retitle. Driven through
//! `rune_fuzz::Session`, pulling the shared fixtures from `merge_common`.
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
use rune_tui::merge::MergeState;

use merge_common::{ch, ctrl, external_write, reprobe, untitled_draft};

/// Opens `/doc.md` seeded with "hello" through the checked driver and
/// returns the session with the opened document's id and the untitled draft
/// it opened alongside (a switch target for the probe-triggering
/// away-and-back below).
fn open_hello() -> (Session, DocumentId, DocumentId) {
    let session = Session::open("/doc.md", "hello");
    let doc_id = session.app().active;
    let draft_id = untitled_draft(session.app(), doc_id);
    (session, doc_id, draft_id)
}

/// Plan WP3 "Done when" (a): a diverged fixture, entered via `^M`, installs
/// the working form as ONE journaled edit, retitles the tab, and undoes
/// byte-for-byte.
#[test]
fn merge_on_a_diverged_document_installs_markers_as_one_journal_step() {
    let (mut session, doc_id, draft_id) = open_hello();
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Clean)
    );

    // Ours changes (one journaled edit) — the buffer diverges from the
    // ancestor the `Load` recorded.
    assert!(session.key(ch('!')).is_none());
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        "!hello"
    );
    // Deliver the typed edit's own `AppendEdit` ack before reprobing — left
    // undelivered, it would still be sitting in `app.db_ops` ahead of the
    // probe below, and the oldest-first `deliver_db` would deliver it
    // instead of the probe.
    assert!(session.deliver_db().is_none());
    let pre_merge_bytes = session
        .app()
        .doc(doc_id)
        .unwrap()
        .buffer
        .content()
        .to_string();
    let journal_pos_before_merge = session.app().doc(doc_id).unwrap().journal.pos();

    // Theirs changes too — both sides moved since the ancestor: Diverged.
    external_write(session.app().vfs.as_ref(), b"disk changed this");
    reprobe(&mut session, draft_id, doc_id);
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Diverged)
    );

    assert!(session.key(ctrl('m')).is_none());
    assert!(matches!(session.app().merge, MergeState::Pending { .. }));
    assert!(session.deliver_db().is_none());

    let doc = session.app().doc(doc_id).unwrap();
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
    assert!(matches!(session.app().merge, MergeState::Active { .. }));
    assert_eq!(
        session.app().doc(doc_id).unwrap().journal.pos(),
        journal_pos_before_merge + 1,
        "merge entry must push exactly one new journal step"
    );

    // Undo mid-merge is not reachable through the key pipeline (the
    // resolver's intercept swallows `⌘Z`), so the command entry point is
    // the seam under test here.
    rune_tui::commands::edit::undo(session.app_mut(), doc_id);
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        pre_merge_bytes,
        "one undo must restore the pre-merge buffer byte-for-byte"
    );
}

/// Plan WP3 "Done when" (b): a `DiskAhead` document with a CLEAN buffer
/// takes the zero-conflict fast path — the resolver never appears, the
/// buffer ends up byte-identical to disk, and merge mode never activates.
#[test]
fn merge_on_a_disk_ahead_clean_document_installs_disk_bytes_with_no_markers() {
    let (mut session, doc_id, draft_id) = open_hello();
    let journal_pos_before_merge = session.app().doc(doc_id).unwrap().journal.pos();

    external_write(session.app().vfs.as_ref(), b"hello world");
    reprobe(&mut session, draft_id, doc_id);
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::DiskAhead)
    );

    assert!(session.key(ctrl('m')).is_none());
    assert!(session.deliver_db().is_none());

    let doc = session.app().doc(doc_id).unwrap();
    assert_eq!(doc.buffer.content(), "hello world");
    assert!(!doc.buffer.content().contains("<<<<<<<"));
    assert_eq!(
        doc.journal.pos(),
        journal_pos_before_merge + 1,
        "the clean fast path is still exactly one journal step"
    );
    assert_eq!(session.app().merge, MergeState::Inactive);
}

/// Plan WP3 "Done when" (c): the on-disk file is not valid UTF-8 — merge
/// entry refuses outright, with feedback, and never touches the buffer.
#[test]
fn merge_refuses_when_the_disk_file_is_not_valid_utf8() {
    let (mut session, doc_id, draft_id) = open_hello();

    external_write(session.app().vfs.as_ref(), &[0xff, 0xfe, 0x00, 0x01]);
    reprobe(&mut session, draft_id, doc_id);
    assert!(matches!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::DiskAhead) | Some(SyncKind::Diverged)
    ));

    assert!(session.key(ctrl('m')).is_none());
    assert!(session.deliver_db().is_none());

    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        "hello",
        "a UTF-8 refusal must never touch the buffer"
    );
    assert!(
        rune_tui::messages::newest_text(session.app())
            .unwrap_or_default()
            .contains("not valid UTF-8"),
        "expected a UTF-8 refusal status, got {:?}",
        rune_tui::messages::newest_text(session.app())
    );
    assert_eq!(session.app().merge, MergeState::Inactive);
}

/// `^M` pressed with no divergence hinted at all must refuse immediately,
/// with feedback, and never enqueue a `MergePrep`.
#[test]
fn merge_with_no_divergence_hint_refuses_without_enqueueing() {
    let (mut session, doc_id, _draft_id) = open_hello();
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Clean)
    );

    assert!(session.key(ctrl('m')).is_none());

    assert_eq!(session.app().merge, MergeState::Inactive);
    assert!(
        session.app().db_ops.is_empty(),
        "no MergePrep should be enqueued"
    );
    assert!(
        rune_tui::messages::newest_text(session.app())
            .unwrap_or_default()
            .contains("no divergence to merge")
    );
}
