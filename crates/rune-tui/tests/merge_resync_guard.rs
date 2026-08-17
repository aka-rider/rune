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
use rune_tui::keymap::KeyCode;
use rune_tui::merge::{MergeState, Resolution};
use rune_tui::workspace;

use merge_common::{
    bare, ch, ctrl, external_write, reprobe, sup, take_ours, take_theirs, untitled_draft,
};

const ANCESTOR: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\n";
const THEIRS: &[u8] = b"one disk\ntwo\nthree\nfour\nfive disk\nsix\nseven\neight\nnine disk\n";

fn enter_three_conflict_merge() -> (Session, DocumentId) {
    let mut session = Session::open("/doc.md", ANCESTOR);
    let doc_id = session.app().active;
    let draft_id = untitled_draft(session.app(), doc_id);

    assert!(session.key(ch('X')).is_none());
    for _ in 0..4 {
        assert!(session.key(bare(KeyCode::Down)).is_none());
    }
    assert!(session.key(bare(KeyCode::Home)).is_none());
    assert!(session.key(ch('X')).is_none());
    for _ in 0..4 {
        assert!(session.key(bare(KeyCode::Down)).is_none());
    }
    assert!(session.key(bare(KeyCode::Home)).is_none());
    assert!(session.key(ch('X')).is_none());
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        "Xone\ntwo\nthree\nfour\nXfive\nsix\nseven\neight\nXnine\n"
    );
    assert!(session.deliver_db_all().is_none());

    external_write(session.app().vfs.as_ref(), THEIRS);
    reprobe(&mut session, draft_id, doc_id);
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Diverged)
    );

    assert!(session.key(ctrl('m')).is_none());
    assert!(session.deliver_db().is_none());

    let MergeState::Active { session: merge, .. } = &session.app().merge else {
        panic!("expected an active resolver, got {:?}", session.app().merge);
    };
    assert_eq!(
        merge.conflicts.len(),
        3,
        "fixture must produce three conflicts"
    );
    assert_eq!(merge.cur, 0);
    (session, doc_id)
}

fn resolution_of(session: &Session, idx: usize) -> Resolution {
    let MergeState::Active { session: merge, .. } = &session.app().merge else {
        panic!("resolver not active");
    };
    merge.conflicts[idx].block.resolution
}

fn cur_of(session: &Session) -> usize {
    let MergeState::Active { session: merge, .. } = &session.app().merge else {
        panic!("resolver not active");
    };
    merge.cur
}

#[test]
fn undo_after_two_takes_reopens_the_last_taken_hunk() {
    let (mut session, doc_id) = enter_three_conflict_merge();

    assert!(session.key(take_theirs()).is_none());
    let pos_after_first_take = session.app().doc(doc_id).unwrap().journal.pos();
    assert!(session.key(take_theirs()).is_none());
    assert_eq!(resolution_of(&session, 0), Resolution::TookTheirs);
    assert_eq!(resolution_of(&session, 1), Resolution::TookTheirs);
    assert_eq!(resolution_of(&session, 2), Resolution::Unresolved);
    assert_eq!(cur_of(&session), 2);

    rune_tui::commands::edit::undo(session.app_mut(), doc_id);
    assert_eq!(
        session.app().doc(doc_id).unwrap().journal.pos(),
        pos_after_first_take,
        "undo must reverse exactly the second take's one journal step"
    );

    assert_eq!(
        resolution_of(&session, 1),
        Resolution::Unresolved,
        "the just-undone hunk must be unresolved again"
    );
    assert_eq!(
        cur_of(&session),
        1,
        "cur must land back on the reopened hunk"
    );
    assert_eq!(
        resolution_of(&session, 0),
        Resolution::TookTheirs,
        "the OTHER take must be untouched"
    );
}

#[test]
fn a_flag_only_kept_ours_survives_an_unrelated_undo() {
    let (mut session, doc_id) = enter_three_conflict_merge();

    assert!(session.key(take_ours()).is_none());
    assert_eq!(resolution_of(&session, 0), Resolution::KeptOurs);
    let pos_before_take = session.app().doc(doc_id).unwrap().journal.pos();
    assert!(session.key(take_theirs()).is_none());
    assert_eq!(resolution_of(&session, 1), Resolution::TookTheirs);

    rune_tui::commands::edit::undo(session.app_mut(), doc_id);
    assert_eq!(
        session.app().doc(doc_id).unwrap().journal.pos(),
        pos_before_take,
        "undo must reverse exactly the take's one journal step"
    );

    assert_eq!(
        resolution_of(&session, 1),
        Resolution::Unresolved,
        "the just-undone take must be unresolved again"
    );
    assert_eq!(
        resolution_of(&session, 0),
        Resolution::KeptOurs,
        "the flag-only kept-ours has unchanged bytes and must keep its state"
    );
}

#[test]
fn undo_and_redo_round_trip_a_take_byte_for_byte() {
    let (mut session, doc_id) = enter_three_conflict_merge();

    let pre_take_bytes = session
        .app()
        .doc(doc_id)
        .unwrap()
        .buffer
        .content()
        .to_string();
    assert!(session.key(take_theirs()).is_none());
    let post_take_bytes = session
        .app()
        .doc(doc_id)
        .unwrap()
        .buffer
        .content()
        .to_string();
    assert!(post_take_bytes.starts_with("one disk\n"));

    assert!(session.key(sup('z')).is_none());
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        pre_take_bytes,
        "undo must restore the pre-take bytes byte-for-byte"
    );
    assert_eq!(resolution_of(&session, 0), Resolution::Unresolved);

    assert!(session.key(ctrl('y')).is_none());
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        post_take_bytes,
        "redo must reapply the take byte-for-byte"
    );
    assert_eq!(resolution_of(&session, 0), Resolution::TookTheirs);
}

#[test]
fn tab_switch_mid_merge_exits_merge_keeps_bytes_and_reverts_title() {
    let (mut session, doc_id) = enter_three_conflict_merge();
    let other = session
        .app_mut()
        .open_document(rune_core::buffer::Buffer::new("scratch"));

    let bytes_before = session
        .app()
        .doc(doc_id)
        .unwrap()
        .buffer
        .content()
        .to_string();
    assert!(
        session
            .app()
            .doc(doc_id)
            .unwrap()
            .file_name()
            .ends_with(": editor <-> disk")
    );

    workspace::switch_to(session.app_mut(), other);

    assert_eq!(session.app().merge, MergeState::Inactive);
    assert!(
        session.app().diff.is_none(),
        "auto-exit must tear the pane view down"
    );
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        bytes_before,
        "auto-exit must never touch the buffer"
    );
    assert!(
        !session
            .app()
            .doc(doc_id)
            .unwrap()
            .file_name()
            .ends_with(": editor <-> disk"),
        "the title must revert on auto-exit"
    );
}

#[test]
fn tab_switch_while_merge_prep_is_still_pending_cancels_with_a_status_and_drops_the_stale_ack() {
    let mut session = Session::open("/doc.md", "hello");
    let doc_id = session.app().active;
    let draft_id = untitled_draft(session.app(), doc_id);

    assert!(session.key(ch('!')).is_none());
    assert!(session.deliver_db().is_none());
    external_write(session.app().vfs.as_ref(), b"disk changed this");
    reprobe(&mut session, draft_id, doc_id);
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Diverged)
    );

    assert!(session.key(ctrl('m')).is_none());
    assert!(
        matches!(session.app().merge, MergeState::Pending { doc, .. } if doc == doc_id),
        "expected a Pending merge attempt, got {:?}",
        session.app().merge
    );

    workspace::switch_to(session.app_mut(), draft_id);

    assert_eq!(
        session.app().merge,
        MergeState::Inactive,
        "a Pending attempt must be cancelled, not left dangling"
    );
    assert!(
        rune_tui::messages::newest_text(session.app())
            .unwrap_or_default()
            .contains("cancelled"),
        "expected a cancellation status, got {:?}",
        rune_tui::messages::newest_text(session.app())
    );

    assert!(session.deliver_db().is_none());
    assert_eq!(session.app().merge, MergeState::Inactive);
    assert_eq!(session.app().active, draft_id);
}
