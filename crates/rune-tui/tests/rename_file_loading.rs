//! A file opened from the Explorer is still waiting for its recovery `Load`
//! when the user renames it: the rename waits for that load, then moves the
//! file through the store, or through a plain rename once the store is gone.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod rename_common;

use std::path::Path;

use rune_db::{DbEvent, OpOutcome};
use rune_fuzz::Session;
use rune_tui::rename::RenameState;
use rune_tui::runtime::Effects;
use rune_tui::workspace;
use rune_vfs::Vfs;

use rename_common::{
    DOC_CONTENT, DOC_PATH, awaits_bind, commit_name, drain_materialize_round_trip,
    kill_store_through, loading_session, store_bound_other_than, sup_key,
};

const NEW_PATH: &str = "/root/b.md";

fn row_for(session: &Session, path: &str) -> (i64, Option<String>) {
    let db = session.app().db.as_ref().expect("store wired");
    let op = db.store.load(Path::new(path)).expect("enqueue load");
    match rune_fuzz::driver::wait_for_db_op(&db.bridge, op) {
        DbEvent::Ok {
            result: OpOutcome::Load(load),
            ..
        } => (load.doc_id.0, load.renamed_from),
        other => panic!("unexpected event for load: {other:?}"),
    }
}

fn save_log_is_clean(session: &Session) {
    let log = rune_tui::messages::log_text(session.app());
    assert!(
        !log.contains("save failed"),
        "a save of the renamed file must land, got {log:?}"
    );
}

#[test]
fn renaming_a_file_before_its_load_lands_saves_to_the_new_path_through_the_store() {
    let (mut session, mem) = loading_session();
    let doc = session.app().active;

    commit_name(&mut session, "b");
    assert!(awaits_bind(&session, doc));
    assert_eq!(
        rune_tui::messages::newest_text(session.app()),
        Some("renaming to b.md once crash recovery is ready\u{2026}")
    );
    assert_eq!(
        mem.read(Path::new(DOC_PATH)).unwrap(),
        DOC_CONTENT.as_bytes(),
        "nothing may move before the file's recovery row is loaded"
    );

    assert!(session.deliver_db_all().is_none());
    assert_eq!(session.app().rename, RenameState::Idle);
    assert_eq!(
        session.app().doc(doc).unwrap().path(),
        Some(Path::new(NEW_PATH))
    );
    assert!(session.app().doc(doc).unwrap().is_store_bound());

    assert!(session.type_("more ").is_none());
    assert!(session.deliver_db_all().is_none());
    assert!(session.key(sup_key('s')).is_none());
    drain_materialize_round_trip(&mut session);

    save_log_is_clean(&session);
    let buffer = session
        .app()
        .doc(doc)
        .unwrap()
        .buffer()
        .content()
        .to_string();
    assert_eq!(mem.read(Path::new(NEW_PATH)).unwrap(), buffer.as_bytes());
    assert!(
        mem.read(Path::new(DOC_PATH)).is_err(),
        "the old name is gone"
    );
    assert!(!session.app().doc(doc).unwrap().is_dirty());
    let bound_row = session.app().doc(doc).unwrap().doc_db().unwrap().db_id;
    assert_eq!(
        row_for(&session, NEW_PATH),
        (bound_row, None),
        "the document's recovery row must already sit at the new path"
    );
}

#[test]
fn a_file_renamed_before_its_load_falls_back_to_a_plain_rename_when_the_store_dies() {
    let (mut session, mem) = loading_session();
    let doc = session.app().active;
    let seed = store_bound_other_than(&session, doc);

    commit_name(&mut session, "b");
    assert!(awaits_bind(&session, doc));

    kill_store_through(&mut session, seed);
    assert!(session.deliver().is_none());

    assert_eq!(session.app().rename, RenameState::Idle);
    assert_eq!(
        session.app().doc(doc).unwrap().path(),
        Some(Path::new(NEW_PATH))
    );
    assert_eq!(
        mem.read(Path::new(NEW_PATH)).unwrap(),
        DOC_CONTENT.as_bytes()
    );
    assert!(
        mem.read(Path::new(DOC_PATH)).is_err(),
        "the old name is gone"
    );

    assert!(session.deliver_db_all().is_none());
    assert!(!session.app().doc(doc).unwrap().is_store_bound());

    let tab = session
        .app()
        .documents
        .order()
        .iter()
        .position(|&id| id == doc)
        .unwrap();
    assert!(session.switch_tab_by_index(tab).is_none());
    assert!(session.type_("more ").is_none());
    assert!(session.key(sup_key('s')).is_none());
    assert!(session.deliver().is_none());

    save_log_is_clean(&session);
    let buffer = session
        .app()
        .doc(doc)
        .unwrap()
        .buffer()
        .content()
        .to_string();
    assert_eq!(mem.read(Path::new(NEW_PATH)).unwrap(), buffer.as_bytes());
    assert!(mem.read(Path::new(DOC_PATH)).is_err());
}

#[test]
fn saving_while_a_rename_awaits_the_load_is_refused_with_feedback() {
    let (mut session, mem) = loading_session();
    let doc = session.app().active;
    commit_name(&mut session, "b");
    assert!(session.type_("more ").is_none());

    assert!(session.key(sup_key('s')).is_none());
    assert_eq!(
        rune_tui::messages::newest_text(session.app()),
        Some("can't save while a rename is in flight")
    );
    assert!(!session.app().doc(doc).unwrap().save_in_flight());
    assert!(awaits_bind(&session, doc));

    assert!(session.deliver_db_all().is_none());
    assert!(mem.read(Path::new(DOC_PATH)).is_err());
    assert_eq!(
        mem.read(Path::new(NEW_PATH)).unwrap(),
        DOC_CONTENT.as_bytes(),
        "the refused save wrote nothing; the rename moved the published bytes"
    );
    assert!(session.app().doc(doc).unwrap().is_dirty());
}

#[test]
fn closing_a_file_whose_rename_awaits_its_load_renames_nothing() {
    let (mut session, mem) = loading_session();
    let doc = session.app().active;
    commit_name(&mut session, "b");

    let _ = workspace::close_now(session.app_mut(), doc, &mut Effects::default());
    assert_eq!(session.app().rename, RenameState::Idle);

    assert!(session.deliver_db_all().is_none());
    assert!(session.deliver().is_none());
    assert_eq!(
        mem.read(Path::new(DOC_PATH)).unwrap(),
        DOC_CONTENT.as_bytes()
    );
    assert!(mem.read(Path::new(NEW_PATH)).is_err());
}
