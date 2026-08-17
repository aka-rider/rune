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
use rune_tui::guard::GuardKind;
use rune_tui::keymap::KeyCode;
use rune_tui::merge::{MergeState, Resolution};
use rune_tui::workspace;

use merge_common::{
    bare, ch, ctrl, external_write, reprobe, save_expecting_refusal, sup, take_ours, take_theirs,
    untitled_draft,
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

    assert!(session.key(sup('z')).is_none());
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

    assert!(session.key(sup('z')).is_none());
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

const PRE_MERGE: &str = "Xone\ntwo\nthree\nfour\nXfive\nsix\nseven\neight\nXnine\n";

#[test]
fn undo_across_the_install_abandons_the_merge_and_keeps_the_save_gate() {
    let (mut session, doc_id) = enter_three_conflict_merge();
    let db_id = session.app().doc(doc_id).unwrap().doc_db().unwrap().db_id;
    let baseline = session.app().file_binding(db_id).unwrap().expect_obs;

    assert!(session.key(sup('z')).is_none());
    assert!(session.deliver_db_all().is_none());

    assert_eq!(
        session.app().merge,
        MergeState::Inactive,
        "unwinding the install must retire the merge"
    );
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        PRE_MERGE,
        "the undo must land on the pre-merge bytes"
    );
    let log = rune_tui::messages::log_text(session.app());
    assert!(
        !log.contains("merge complete"),
        "an unwound install is never a completion, log: {log:?}"
    );
    assert!(
        log.contains("merge closed"),
        "the auto-exit must be visible, log: {log:?}"
    );
    assert_eq!(
        session.app().file_binding(db_id).unwrap().expect_obs,
        baseline,
        "the save-CAS baseline must not advance on an unwound install"
    );
    assert!(
        session
            .app()
            .doc(doc_id)
            .unwrap()
            .last_sync
            .is_some_and(SyncKind::is_disk_divergent),
        "the document is still truthfully diverged"
    );

    save_expecting_refusal(&mut session);
    let Some(prompt) = &session.app().guard else {
        panic!(
            "expected the disk-conflict Guard, not a silent overwrite, log: {:?}",
            rune_tui::messages::log_text(session.app())
        );
    };
    assert_eq!(prompt.doc, doc_id);
    assert!(matches!(prompt.kind, GuardKind::DiskConflict));
    assert_eq!(
        session
            .app()
            .vfs
            .read(std::path::Path::new("/doc.md"))
            .unwrap(),
        THEIRS,
        "the refused save must leave the external bytes on disk untouched"
    );
}

#[test]
fn redo_after_an_install_unwind_does_not_resurrect_a_phantom_session() {
    let (mut session, doc_id) = enter_three_conflict_merge();
    let working_form = session
        .app()
        .doc(doc_id)
        .unwrap()
        .buffer
        .content()
        .to_string();

    assert!(session.key(sup('z')).is_none());
    assert!(session.deliver_db_all().is_none());
    assert_eq!(session.app().merge, MergeState::Inactive);

    assert!(session.key(ctrl('y')).is_none());
    assert!(session.deliver_db_all().is_none());

    assert_eq!(
        session.app().merge,
        MergeState::Inactive,
        "redo must not resurrect a session the unwind retired"
    );
    assert!(
        session.app().diff.is_none(),
        "no pane view may come back without a live session"
    );
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        working_form,
        "redo still restores the working-form bytes as an ordinary edit"
    );
}

#[test]
fn hand_edit_resolving_the_last_conflict_posts_a_gate_open_status() {
    let (mut session, _doc_id) = enter_three_conflict_merge();

    assert!(session.key(take_theirs()).is_none());
    assert!(session.key(take_theirs()).is_none());
    assert_eq!(resolution_of(&session, 2), Resolution::Unresolved);

    assert!(session.key(bare(KeyCode::Right)).is_none());
    assert!(session.key(ch('Q')).is_none());

    assert_eq!(resolution_of(&session, 2), Resolution::HandEdited);
    let MergeState::Active { session: merge, .. } = &session.app().merge else {
        panic!("a hand edit must never auto-complete the merge");
    };
    assert_eq!(merge.unresolved_count(), 0);
    assert!(
        rune_tui::messages::newest_text(session.app())
            .unwrap_or_default()
            .contains("all conflicts resolved"),
        "the gate opening silently is invisible, log: {:?}",
        rune_tui::messages::log_text(session.app())
    );
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
