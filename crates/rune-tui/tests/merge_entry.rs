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

use merge_common::{ch, ctrl, external_write, reprobe, sup, untitled_draft};

fn open_hello() -> (Session, DocumentId, DocumentId) {
    let session = Session::open("/doc.md", "hello");
    let doc_id = session.app().active;
    let draft_id = untitled_draft(session.app(), doc_id);
    (session, doc_id, draft_id)
}

#[test]
fn merge_on_a_diverged_document_installs_the_merged_result_as_one_journal_step() {
    let (mut session, doc_id, draft_id) = open_hello();
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Clean)
    );

    assert!(session.key(ch('!')).is_none());
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        "!hello"
    );
    assert!(session.deliver_db().is_none());
    let pre_merge_bytes = session
        .app()
        .doc(doc_id)
        .unwrap()
        .buffer
        .content()
        .to_string();
    let journal_pos_before_merge = session.app().doc(doc_id).unwrap().journal.pos();

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
        !doc.buffer.content().contains("<<<<<<<"),
        "the pane install must hold no conflict markers: {:?}",
        doc.buffer.content()
    );
    assert_eq!(
        doc.buffer.content(),
        "!hello",
        "the conflict shows the ours text in place"
    );
    assert!(
        doc.file_name().ends_with(": editor <-> disk"),
        "tab/title name must carry the merge suffix: {:?}",
        doc.file_name()
    );
    assert!(matches!(session.app().merge, MergeState::Active { .. }));
    let diff = session
        .app()
        .diff
        .as_ref()
        .expect("merge entry must install the pane view");
    assert_eq!(diff.right, doc_id);
    assert_eq!(
        diff.left.buffer.content(),
        "disk changed this",
        "the left pane must hold the disk (theirs) text"
    );
    assert_eq!(
        session.app().doc(doc_id).unwrap().journal.pos(),
        journal_pos_before_merge + 1,
        "merge entry must push exactly one new journal step"
    );

    assert!(session.key(sup('z')).is_none());
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        pre_merge_bytes,
        "one undo must restore the pre-merge buffer byte-for-byte"
    );
}

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
    assert!(
        session.app().diff.is_none(),
        "a zero-conflict merge never opens the pane view"
    );
}

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

#[test]
fn a_second_ctrl_m_while_pending_does_not_clobber_the_ticket() {
    let (mut session, doc_id, draft_id) = open_hello();
    assert!(session.key(ch('!')).is_none());
    assert!(session.deliver_db().is_none());

    external_write(session.app().vfs.as_ref(), b"disk changed this");
    reprobe(&mut session, draft_id, doc_id);
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Diverged)
    );

    assert!(session.key(ctrl('m')).is_none());
    assert!(matches!(session.app().merge, MergeState::Pending { .. }));
    assert_eq!(session.app().db_ops.len(), 1);

    assert!(session.key(ctrl('m')).is_none());
    assert!(
        matches!(session.app().merge, MergeState::Pending { .. }),
        "the second press must not clobber the still-outstanding ticket, got {:?}",
        session.app().merge
    );
    assert_eq!(
        session.app().db_ops.len(),
        1,
        "the second press must not enqueue a second MergePrep"
    );
    assert_eq!(
        rune_tui::messages::newest_text(session.app()),
        Some("merge already preparing")
    );

    assert!(session.deliver_db().is_none());
    assert!(
        matches!(session.app().merge, MergeState::Active { .. }),
        "the FIRST attempt's ack must still land, got {:?}",
        session.app().merge
    );
}

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
