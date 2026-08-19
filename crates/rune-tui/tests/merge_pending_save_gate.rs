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

use merge_common::{ch, ctrl, external_write, messages_posts, reprobe, sup, untitled_draft};

fn diverged_session() -> (Session, DocumentId, DocumentId) {
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
    (session, doc_id, draft_id)
}

fn pending_op_ids(session: &Session) -> Vec<u64> {
    session.app().db_ops.keys().copied().collect()
}

#[test]
fn save_during_merge_pending_is_refused_with_feedback() {
    let (mut session, doc_id, _draft_id) = diverged_session();

    assert!(session.key(ctrl('m')).is_none());
    assert!(
        matches!(session.app().merge, MergeState::Pending { .. }),
        "^M must leave the merge attempt Pending on its disk-state round trip"
    );

    let ops_before = pending_op_ids(&session);
    let posts_before = messages_posts(&session);

    assert!(session.key(sup('s')).is_none());

    assert!(
        !session.app().doc(doc_id).unwrap().save_in_flight(),
        "a save armed during a Pending merge would land a materialize on an \
         unresolved working form"
    );
    assert_eq!(
        pending_op_ids(&session),
        ops_before,
        "no store op may be enqueued by a refused save"
    );
    assert!(
        messages_posts(&session) > posts_before,
        "the refusal must tell the user why nothing was saved"
    );
}

#[test]
fn a_merge_refused_save_still_saves_once_the_merge_is_abandoned() {
    let (mut session, doc_id, _draft_id) = diverged_session();

    assert!(session.key(ctrl('m')).is_none());
    assert!(session.key(sup('s')).is_none());
    assert!(session.deliver_db().is_none());
    assert!(
        matches!(session.app().merge, MergeState::Active { .. }),
        "the prep ack must still land the resolver when no save slipped through"
    );

    assert!(session.key(ctrl('m')).is_none());
    assert_eq!(session.app().merge, MergeState::Inactive);

    assert!(session.key(sup('s')).is_none());
    assert!(
        session.app().doc(doc_id).unwrap().save_in_flight(),
        "with the merge gone the very next ⌘S must save normally"
    );
}
