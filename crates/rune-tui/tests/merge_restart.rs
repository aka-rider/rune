//! Crash re-entry seam tests: a dead session's still-`active` merge row
//! is re-entered on a hydrating load when the journal reconstruction still
//! byte-matches the recorded working form, abandoned when it does not, and
//! left untouched by a binding-only load. Sessions are driven through
//! `rune_fuzz::Session` over one shared `Mem` and a real on-disk `Store`
//! (`db_wiring_common::store_at`/`restarted_store_at`); only the
//! binding-only fixture hand-assembles its `App`, since a prebind load is
//! not reachable through the driver's own `open_path` setup.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod merge_common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_core::buffer::{AppliedEdit, Buffer};
use rune_db::SyncKind;
use rune_fuzz::Session;
use rune_fuzz::driver::wait_for_db_op;
use rune_tui::app::App;
use rune_tui::db::{Db, LoadPurpose, PendingOp};
use rune_tui::document::DocumentId;
use rune_tui::keymap::KeyCode;
use rune_tui::merge::MergeState;
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::{Mem, Vfs};

use merge_common::db_wiring_common::{publish, restarted_store_at, store_at, temp_db_dir};
use merge_common::{
    bare, ch, ctrl, external_write, reprobe, take_ours, take_theirs, untitled_draft,
};

const ANCESTOR: &[u8] = b"one\ntwo\nthree\nfour\nfive\n";
const THEIRS: &[u8] = b"one disk\ntwo\nthree\nfour\nfive disk\n";

fn temp_db_path(label: &str) -> PathBuf {
    temp_db_dir(&format!("merge-restart-{label}")).join("rune-db.sqlite")
}

/// Session A: opens `/doc.md`, edits lines 1 and 5, sees an external
/// rewrite of both, and enters the resolver — two conflicts, none resolved.
fn enter_two_conflict_merge_at(db_path: &Path, mem: &Arc<Mem>) -> (Session, DocumentId) {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let (store, bridge) = store_at(db_path, Arc::clone(&vfs));
    let mut session =
        Session::open_with_db("/doc.md", Arc::clone(mem), Db::new(store, bridge, false));
    let doc_id = session.app().active;
    let draft_id = untitled_draft(session.app(), doc_id);

    assert!(session.key(ch('X')).is_none());
    for _ in 0..4 {
        assert!(session.key(bare(KeyCode::Down)).is_none());
    }
    assert!(session.key(bare(KeyCode::End)).is_none());
    assert!(session.key(ch('Z')).is_none());
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        "Xone\ntwo\nthree\nfour\nfiveZ\n"
    );
    assert!(session.deliver_db_all().is_none());

    external_write(session.app().vfs.as_ref(), THEIRS);
    reprobe(&mut session, draft_id, doc_id);
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Diverged)
    );

    assert!(session.key(ctrl('m')).is_none());
    assert!(session.deliver_db_all().is_none());
    assert!(
        matches!(&session.app().merge, MergeState::Active { session, .. } if session.conflicts.len() == 2),
        "fixture must enter a two-conflict merge, got {:?}",
        session.app().merge
    );
    (session, doc_id)
}

/// Joins the store's threads deterministically before the disk is reopened
/// by the next "session" — the same shutdown `rune-cli`'s exit path runs.
fn shutdown(mut session: Session) {
    session
        .app_mut()
        .db
        .take()
        .expect("store present")
        .shutdown();
}

/// A fresh `Session` over the SAME db path and `Mem`, with the liveness
/// check overridden so the dead session's rows are recoverable — the
/// restart. Its setup runs the hydrating `Load` (and drains its ack), so
/// any merge re-entry has already happened by the time this returns.
fn restart_session(db_path: &Path, mem: &Arc<Mem>) -> Session {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let (store, bridge) = restarted_store_at(db_path, vfs);
    Session::open_with_db("/doc.md", Arc::clone(mem), Db::new(store, bridge, false))
}

#[test]
fn merge_resumes_after_restart() {
    let db_path = temp_db_path("resume");
    let mem = Arc::new(Mem::new());
    publish(mem.as_ref(), Path::new("/doc.md"), ANCESTOR);

    let (mut session_a, _doc_a) = enter_two_conflict_merge_at(&db_path, &mem);
    assert!(session_a.key(take_ours()).is_none());
    assert!(
        matches!(&session_a.app().merge, MergeState::Active { .. }),
        "one of two resolved keeps the resolver active"
    );
    assert!(session_a.deliver_db_all().is_none());
    shutdown(session_a);

    let mut session_b = restart_session(&db_path, &mem);
    let doc_b = session_b.app().active;

    let MergeState::Active {
        session: merge,
        doc,
    } = &session_b.app().merge
    else {
        panic!(
            "expected the merge to resume Active, got {:?}",
            session_b.app().merge
        );
    };
    assert_eq!(*doc, doc_b);
    assert_eq!(
        merge.conflicts.len(),
        2,
        "both blocks travel across the restart"
    );
    assert_eq!(
        merge
            .conflicts
            .iter()
            .filter(|p| !p.block.resolution.is_resolved())
            .count(),
        1,
        "exactly the unresolved block survives as unresolved"
    );
    assert!(
        rune_tui::messages::log_text(session_b.app()).contains("merge resumed — 1 conflict(s)"),
        "expected the resume status, got {:?}",
        rune_tui::messages::log_text(session_b.app())
    );
    assert!(
        !rune_tui::messages::log_text(session_b.app()).contains("[^M]erge to reconcile"),
        "a resumed merge must not also post the stale ^M invitation, got {:?}",
        rune_tui::messages::log_text(session_b.app())
    );
    assert_eq!(
        session_b.app().doc(doc_b).unwrap().buffer.content(),
        "Xone\ntwo\nthree\nfour\nfiveZ\n",
        "the working form re-hydrates byte-for-byte, markers-free"
    );
    assert!(
        session_b.app().diff.is_some(),
        "the resumed merge re-installs the pane view"
    );
    assert!(
        session_b
            .app()
            .doc(doc_b)
            .unwrap()
            .file_name()
            .ends_with(": editor <-> disk"),
        "the resolver's title returns with the resumed merge"
    );

    assert!(session_b.key(take_theirs()).is_none());
    assert!(session_b.deliver_db_all().is_none());
    assert_eq!(
        session_b.app().merge,
        MergeState::Inactive,
        "resolving the survivor completes the resumed merge"
    );
    assert_eq!(
        session_b.app().doc(doc_b).unwrap().buffer.content(),
        "Xone\ntwo\nthree\nfour\nfive disk\n",
        "the survivor's take-disk lands in the buffer"
    );
    shutdown(session_b);
}

#[test]
fn journaled_edit_past_the_install_abandons_the_merge_on_restart() {
    let db_path = temp_db_path("abandon");
    let mem = Arc::new(Mem::new());
    publish(mem.as_ref(), Path::new("/doc.md"), ANCESTOR);

    let (session_a, doc_a) = enter_two_conflict_merge_at(&db_path, &mem);
    let db_id = session_a.app().doc(doc_a).unwrap().doc_db().unwrap().db_id;
    session_a
        .app()
        .db
        .as_ref()
        .unwrap()
        .store
        .append_edit(
            rune_db::DocId(db_id),
            rune_db::BindingToken::next(),
            rune_db::Seq(0),
            &[AppliedEdit {
                start: 0,
                end: 0,
                deleted: String::new(),
                insert: "STRAY ".to_string(),
            }],
            &[],
            &[],
        )
        .expect("journal a stray edit past the install");
    shutdown(session_a);

    let session_b = restart_session(&db_path, &mem);
    let doc_b = session_b.app().active;

    assert_eq!(
        session_b.app().merge,
        MergeState::Inactive,
        "a reconstruction that no longer matches the recorded form must not re-enter"
    );
    assert!(
        !rune_tui::messages::log_text(session_b.app()).contains("merge resumed"),
        "no resume message may be posted"
    );
    let content = session_b
        .app()
        .doc(doc_b)
        .unwrap()
        .buffer
        .content()
        .to_string();
    assert_eq!(
        content, "STRAY Xone\ntwo\nthree\nfour\nfiveZ\n",
        "the recovered draft keeps the journaled bytes as plain text"
    );

    // The row was flipped to abandoned: a second full load must not
    // re-offer re-entry either.
    shutdown(session_b);
    let session_c = restart_session(&db_path, &mem);
    assert_eq!(session_c.app().merge, MergeState::Inactive);
    shutdown(session_c);
}

#[test]
fn binding_only_load_installs_nothing_and_leaves_the_row_active() {
    let db_path = temp_db_path("binding-only");
    let mem = Arc::new(Mem::new());
    publish(mem.as_ref(), Path::new("/doc.md"), ANCESTOR);
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;

    let (session_a, _doc_a) = enter_two_conflict_merge_at(&db_path, &mem);
    shutdown(session_a);

    let disk = String::from_utf8(vfs.read(Path::new("/doc.md")).unwrap()).unwrap();
    let (store_b, bridge_b) = restarted_store_at(&db_path, Arc::clone(&vfs));
    let mut app_b = App::new(
        Buffer::new(&disk),
        Some(PathBuf::from("/doc.md")),
        Arc::clone(&vfs),
        Some(Db::new(store_b, Arc::clone(&bridge_b), false)),
    );
    let doc_b = app_b.active;
    let issued_version = app_b.doc(doc_b).unwrap().buffer.version();

    let op_id = app_b
        .db
        .as_ref()
        .unwrap()
        .store
        .load(Path::new("/doc.md"))
        .expect("enqueue binding-only load");
    app_b.db_ops.insert(
        op_id,
        PendingOp::load(
            doc_b,
            issued_version,
            LoadPurpose::Rebaseline { expect_row: None },
        ),
    );
    let evt = wait_for_db_op(&bridge_b, op_id);
    let mut effects = Effects::default();
    rune_tui::app::update(&mut app_b, Msg::Db(evt), &mut effects);

    assert_eq!(
        app_b.merge,
        MergeState::Inactive,
        "a binding-only load must install no merge state"
    );
    assert!(
        !rune_tui::messages::log_text(&app_b).contains("merge resumed"),
        "no resume message may be posted for a skipped hydration"
    );
    assert_eq!(
        app_b.doc(doc_b).unwrap().buffer.content(),
        disk,
        "a binding-only load must not touch the buffer"
    );

    // The row stayed active: the next FULL load re-offers and re-enters.
    let issued_version = app_b.doc(doc_b).unwrap().buffer.version();
    let op_id = app_b
        .db
        .as_ref()
        .unwrap()
        .store
        .load(Path::new("/doc.md"))
        .expect("enqueue full load");
    app_b.db_ops.insert(
        op_id,
        PendingOp::load(doc_b, issued_version, LoadPurpose::Recover),
    );
    let evt = wait_for_db_op(&bridge_b, op_id);
    let mut effects = Effects::default();
    rune_tui::app::update(&mut app_b, Msg::Db(evt), &mut effects);

    assert!(
        matches!(
            &app_b.merge,
            MergeState::Active { session: merge, doc }
                if *doc == doc_b && merge.conflicts.iter().filter(|p| !p.block.resolution.is_resolved()).count() == 2
        ),
        "the full load must re-enter the still-active merge, got {:?}",
        app_b.merge
    );
    while let Some(&op_id) = app_b.db_ops.keys().min() {
        let evt = wait_for_db_op(&bridge_b, op_id);
        let mut effects = Effects::default();
        rune_tui::app::update(&mut app_b, Msg::Db(evt), &mut effects);
    }
    app_b.db.take().expect("store present").shutdown();
}
