//! A draft minted mid-session is still waiting for its recovery row when the
//! user names it: the name waits for that row, then creates the file
//! through the store, or through a plain create once the store is gone.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod rename_common;

use std::path::PathBuf;

use rune_db::{DbEvent, OpOutcome};
use rune_fuzz::Session;
use rune_tui::db::PublishMode;
use rune_tui::pane::Pane;
use rune_tui::rename::RenameState;
use rune_tui::runtime::Effects;
use rune_tui::workspace;
use rune_vfs::Vfs;

use rename_common::{
    awaits_bind, draft_session, drain_materialize_round_trip, kill_store_through, name_draft,
    store_bound_other_than, sup_key,
};

fn create_target(session: &Session) -> PathBuf {
    rune_tui::explorer::initial_root(session.app()).join("fresh.md")
}

fn recoverable_scratch_rows(session: &Session) -> Vec<i64> {
    let db = session.app().db.as_ref().expect("store wired");
    let op = db
        .store
        .recoverable_scratch(0)
        .expect("enqueue recoverable_scratch");
    match rune_fuzz::driver::wait_for_db_op(&db.bridge, op) {
        DbEvent::Ok {
            result: OpOutcome::Ids(ids),
            ..
        } => ids,
        other => panic!("unexpected event for recoverable_scratch: {other:?}"),
    }
}

#[test]
fn naming_a_draft_before_its_scratch_ack_binds_through_the_store() {
    let (mut session, mem) = draft_session();
    let draft = session.app().active;
    assert!(session.type_("draft body").is_none());
    let target = create_target(&session);

    name_draft(&mut session, "fresh");
    assert!(
        mem.read(&target).is_err(),
        "nothing may be created before the draft's recovery row exists"
    );
    assert!(awaits_bind(&session, draft));
    assert_eq!(session.app().title.text(), "fresh.md");

    assert!(session.deliver_db_all().is_none());
    drain_materialize_round_trip(&mut session);

    assert_eq!(session.app().rename, RenameState::Idle);
    assert_eq!(mem.read(&target).unwrap(), b"draft body");
    let doc = session.app().doc(draft).unwrap();
    assert!(doc.is_store_bound());
    assert_eq!(
        doc.doc_db().unwrap().publish_mode,
        PublishMode::OverwriteExisting
    );
    assert_eq!(doc.file_name(), "fresh.md");

    assert!(session.type_(" and more").is_none());
    assert!(session.deliver_db_all().is_none());
    assert!(session.key(sup_key('s')).is_none());
    drain_materialize_round_trip(&mut session);

    let log = rune_tui::messages::log_text(session.app());
    assert!(
        !log.contains("created by something else"),
        "a save of the draft's own file must never lose a create race, got {log:?}"
    );
    assert_eq!(mem.read(&target).unwrap(), b"draft body and more");
    assert!(!session.app().doc(draft).unwrap().is_dirty());
    assert!(
        recoverable_scratch_rows(&session).is_empty(),
        "the draft's recovery row must follow it to its file, never linger as an untitled draft"
    );
}

#[test]
fn a_draft_named_before_its_ack_falls_back_to_a_plain_create_when_the_store_dies() {
    let (mut session, mem) = draft_session();
    let draft = session.app().active;
    let seed = store_bound_other_than(&session, draft);
    assert!(session.type_("draft body").is_none());
    let target = create_target(&session);

    name_draft(&mut session, "fresh");
    assert!(awaits_bind(&session, draft));

    kill_store_through(&mut session, seed);

    assert!(session.deliver().is_none());
    assert_eq!(session.app().rename, RenameState::Idle);
    assert_eq!(mem.read(&target).unwrap(), b"draft body");
    let doc = session.app().doc(draft).unwrap();
    assert_eq!(doc.file_name(), "fresh.md");

    assert!(session.deliver_db_all().is_none());
    assert!(
        !session.app().doc(draft).unwrap().is_store_bound(),
        "a recovery row arriving after the plain create must not bind a create-only replica to the file"
    );
}

#[test]
fn saving_while_a_draft_name_awaits_binding_is_refused_with_feedback() {
    let (mut session, mem) = draft_session();
    let draft = session.app().active;
    assert!(session.type_("draft body").is_none());
    let target = create_target(&session);
    name_draft(&mut session, "fresh");

    assert!(session.key(sup_key('s')).is_none());
    assert!(
        rune_tui::messages::newest_text(session.app())
            .is_some_and(|m| m.contains("can't save while a rename is in flight")),
        "got {:?}",
        rune_tui::messages::newest_text(session.app())
    );
    assert_ne!(session.app().focus(), Pane::Title);
    assert!(awaits_bind(&session, draft));

    assert!(session.deliver_db_all().is_none());
    drain_materialize_round_trip(&mut session);
    assert_eq!(mem.read(&target).unwrap(), b"draft body");
}

#[test]
fn closing_a_draft_whose_name_awaits_binding_creates_nothing() {
    let (mut session, mem) = draft_session();
    let draft = session.app().active;
    assert!(session.type_("draft body").is_none());
    let target = create_target(&session);
    name_draft(&mut session, "fresh");

    let _ = workspace::close_now(session.app_mut(), draft, &mut Effects::default());
    assert_eq!(session.app().rename, RenameState::Idle);

    assert!(session.deliver_db_all().is_none());
    assert!(session.deliver().is_none());
    assert!(mem.read(&target).is_err());
}
