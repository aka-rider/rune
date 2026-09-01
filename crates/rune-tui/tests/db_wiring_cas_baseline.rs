#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod merge_common;

use std::path::Path;

use rune_core::buffer::Buffer;
use rune_db::SyncKind;
use rune_fuzz::Session;
use rune_tui::app::{self, App};
use rune_tui::db::DocDb;
use rune_tui::footer::footer_text;
use rune_tui::runtime::Effects;
use rune_tui::workspace;

use merge_common::{ch, deliver_op_unchecked, external_write, save_and_ack, sup, untitled_draft};

fn bind_second_tab(
    app: &mut App,
    db_id: i64,
    path: &Path,
    content: &str,
) -> rune_tui::document::DocumentId {
    let resolved = rune_tui::resolved::ResolvedPath::resolve(app.vfs.as_ref(), path)
        .expect("the seeded path resolves");
    let id = app.open_document_bound(Buffer::new(content), resolved);
    {
        let doc = app.doc_mut(id).unwrap();
        doc.set_doc_db_for_test(DocDb::new(
            db_id,
            rune_tui::db::PublishMode::OverwriteExisting,
            rune_db::Seq(0),
        ));
    }
    app.install_or_join_file_binding(db_id, None);
    id
}

#[test]
fn stale_probe_verdict_for_tab_b_is_discarded_then_a_fresh_probe_lands_the_real_one() {
    let mut session = Session::open("/doc.md", "hello");
    let id_a = session.app().active;
    let db_id = session.app().doc(id_a).unwrap().doc_db().unwrap().db_id;
    let id_b = bind_second_tab(session.app_mut(), db_id, Path::new("/doc.md"), "hello");
    session.app_mut().doc_mut(id_b).unwrap().last_sync = Some(SyncKind::DiskAhead);

    workspace::switch_to(session.app_mut(), id_b);
    let stale_probe_op = *session
        .app()
        .db_ops
        .iter()
        .find(|(_, pending)| pending.doc == id_b && pending.is_probe)
        .expect("probe enqueued for tab b")
        .0;

    workspace::switch_to(session.app_mut(), id_a);
    let a_probe = *session
        .app()
        .db_ops
        .iter()
        .find(|(_, pending)| pending.doc == id_a && pending.is_probe)
        .expect("probe enqueued for tab a")
        .0;
    deliver_op_unchecked(&mut session, a_probe);

    assert!(session.key(ch('!')).is_none());
    let edit_op = *session
        .app()
        .db_ops
        .keys()
        .find(|id| **id != stale_probe_op)
        .expect("append-edit op enqueued");
    deliver_op_unchecked(&mut session, edit_op);

    assert!(session.key(sup('s')).is_none());
    let prepare_op = *session
        .app()
        .db_ops
        .keys()
        .find(|id| **id != stale_probe_op)
        .expect("materialize-prepare op enqueued");
    let prepare_effects = deliver_op_unchecked(&mut session, prepare_op);
    let save_cmd = prepare_effects
        .cmds
        .into_iter()
        .find(|c| c.kind() == rune_tui::runtime::CmdKind::Save)
        .expect("the prepare ack must spawn the caller-side vfs Cmd");
    let vfs_done_msg = save_cmd.run().expect("the vfs Cmd must reply");
    let mut effects = Effects::default();
    app::update(session.app_mut(), vfs_done_msg, &mut effects);
    let record_op = *session
        .app()
        .db_ops
        .keys()
        .find(|id| **id != stale_probe_op)
        .expect("materialize-record op enqueued");
    deliver_op_unchecked(&mut session, record_op);

    assert_eq!(
        session.app().file_binding(db_id).unwrap().baseline_epoch,
        1,
        "tab A's committed save must have bumped the shared save epoch"
    );
    assert_eq!(
        session.app().db_ops.len(),
        1,
        "only tab b's pre-save probe is still outstanding"
    );

    deliver_op_unchecked(&mut session, stale_probe_op);

    assert_eq!(
        session.app().doc(id_b).unwrap().last_sync,
        Some(SyncKind::DiskAhead),
        "the stale verdict must never overwrite last_sync"
    );
    let reissued: Vec<_> = session
        .app()
        .db_ops
        .iter()
        .filter(|(_, pending)| pending.doc == id_b && pending.is_probe)
        .map(|(&id, _)| id)
        .collect();
    assert_eq!(
        reissued.len(),
        1,
        "exactly one fresh probe must be re-issued for tab b"
    );
    let fresh_op = reissued[0];
    assert_ne!(
        fresh_op, stale_probe_op,
        "the reissued probe must be a NEW op, not the stale one retried"
    );

    deliver_op_unchecked(&mut session, fresh_op);
    assert_eq!(
        session.app().doc(id_b).unwrap().last_sync,
        Some(SyncKind::Clean),
        "the fresh post-save verdict must land: the shared row's journal already reconstructs \
         to tab A's own published bytes, matching disk"
    );
}

#[test]
fn external_disk_change_under_a_dirty_tab_surfaces_as_diverged_without_touching_the_buffer() {
    let mut session = Session::open("/doc.md", "hello");
    let doc_id = session.app().active;
    let draft_id = untitled_draft(session.app(), doc_id);

    assert!(session.key(ch('!')).is_none());
    assert!(session.deliver_db().is_none());
    let dirty_buffer = session
        .app()
        .doc(doc_id)
        .unwrap()
        .buffer
        .content()
        .to_string();
    assert_eq!(dirty_buffer, "!hello");

    external_write(session.app().vfs.as_ref(), b"someone else's edit");

    workspace::switch_to(session.app_mut(), draft_id);
    workspace::switch_to(session.app_mut(), doc_id);
    assert!(session.deliver_db().is_none());

    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Diverged),
        "an edited buffer under a moved disk must classify as Diverged, never silently reconciled"
    );
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        dirty_buffer,
        "the probe must never overwrite the user's own unsaved edit"
    );
    assert!(
        footer_text(session.app()).contains("disk changed"),
        "the conflict must surface to the user: {:?}",
        footer_text(session.app())
    );
}

#[test]
fn probe_after_a_lost_materialize_record_recognizes_its_own_bytes_as_clean_not_diverged() {
    let mut session = Session::open("/doc.md", "hello");
    let doc_id = session.app().active;
    let draft_id = untitled_draft(session.app(), doc_id);
    let db_id = session.app().doc(doc_id).unwrap().doc_db().unwrap().db_id;

    assert!(session.key(ch('!')).is_none());
    assert!(session.deliver_db().is_none());
    let content = session
        .app()
        .doc(doc_id)
        .unwrap()
        .buffer
        .content()
        .to_string();

    external_write(session.app().vfs.as_ref(), content.as_bytes());
    session
        .app_mut()
        .file_binding_mut(db_id)
        .unwrap()
        .pending_rebaseline_hash = Some(rune_db::hash_bytes(content.as_bytes()));

    workspace::switch_to(session.app_mut(), draft_id);
    workspace::switch_to(session.app_mut(), doc_id);
    assert!(session.deliver_db().is_none());

    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Clean),
        "the session's own lost-bookkeeping echo must classify as Clean, not a conflict"
    );
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        content,
        "a probe must never rewrite the buffer even when it heals the disk fact"
    );
    assert!(
        session.app().guard.is_none(),
        "a self-echoed disk fact must never raise the disk-conflict Guard"
    );
}

#[test]
fn resaving_the_sessions_own_lost_bookkeeping_echo_does_not_manufacture_a_disk_conflict() {
    let mut session = Session::open("/doc.md", "hello");
    let doc_id = session.app().active;
    let db_id = session.app().doc(doc_id).unwrap().doc_db().unwrap().db_id;

    assert!(session.key(ch('!')).is_none());
    assert!(session.deliver_db().is_none());
    let content = session
        .app()
        .doc(doc_id)
        .unwrap()
        .buffer
        .content()
        .to_string();

    external_write(session.app().vfs.as_ref(), content.as_bytes());
    session
        .app_mut()
        .file_binding_mut(db_id)
        .unwrap()
        .pending_rebaseline_hash = Some(rune_db::hash_bytes(content.as_bytes()));

    save_and_ack(&mut session);

    assert!(
        session.app().guard.is_none(),
        "re-publishing the session's own lost-bookkeeping echo must not read as a disk conflict"
    );
    assert_eq!(
        session.app().vfs.read(Path::new("/doc.md")).unwrap(),
        content.as_bytes()
    );
}

#[test]
fn a_probe_requested_while_a_save_is_in_flight_is_deferred_and_fires_once_per_bound_document_after_it_resolves()
 {
    let mut session = Session::open("/doc.md", "hello");
    let id_a = session.app().active;
    let db_id = session.app().doc(id_a).unwrap().doc_db().unwrap().db_id;
    let id_b = bind_second_tab(session.app_mut(), db_id, Path::new("/doc.md"), "hello");

    assert!(session.key(ch('!')).is_none());
    assert!(session.deliver_db().is_none());

    assert!(session.key(sup('s')).is_none());
    let prepare_op = *session
        .app()
        .db_ops
        .keys()
        .next()
        .expect("materialize-prepare op enqueued");

    workspace::switch_to(session.app_mut(), id_b);
    workspace::switch_to(session.app_mut(), id_a);

    assert!(
        session.app().file_binding(db_id).unwrap().pending_probe,
        "a probe requested while a save is in flight must be deferred, not enqueued"
    );
    assert_eq!(
        session.app().db_ops.len(),
        1,
        "no probe op must be enqueued while the save is still in flight"
    );

    let prepare_effects = deliver_op_unchecked(&mut session, prepare_op);
    let save_cmd = prepare_effects
        .cmds
        .into_iter()
        .find(|c| c.kind() == rune_tui::runtime::CmdKind::Save)
        .expect("the prepare ack must spawn the caller-side vfs Cmd");
    let vfs_done_msg = save_cmd.run().expect("the vfs Cmd must reply");
    let mut effects = Effects::default();
    app::update(session.app_mut(), vfs_done_msg, &mut effects);
    let record_op = *session
        .app()
        .db_ops
        .keys()
        .next()
        .expect("materialize-record op enqueued");
    deliver_op_unchecked(&mut session, record_op);

    assert!(
        !session.app().file_binding(db_id).unwrap().pending_probe,
        "the deferred flag must be cleared once the save resolves"
    );
    let probes: Vec<_> = session
        .app()
        .db_ops
        .values()
        .filter(|pending| pending.is_probe)
        .map(|pending| pending.doc)
        .collect();
    assert_eq!(
        probes.len(),
        2,
        "exactly one fresh probe must fire per document bound to db_id"
    );
    assert!(probes.contains(&id_a));
    assert!(probes.contains(&id_b));

    assert!(session.deliver_db_all().is_none());
}
