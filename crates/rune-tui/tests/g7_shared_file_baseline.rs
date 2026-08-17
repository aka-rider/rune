//! Integration tests for the shared save-CAS baseline: `expect_obs`/
//! `pending_rebaseline_hash`/`baseline_epoch` are shared per `db_id`
//! (`App::file_bindings`), not copied per `Document` — two `Document`s
//! (tabs) bound to the SAME underlying file must see the one truth about
//! what disk holds. Driven through `rune_fuzz::Session`, pulling shared
//! fixtures from `merge_common`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod merge_common;

use std::path::Path;

use rune_core::buffer::Buffer;
use rune_db::{DbEvent, OpOutcome, SyncKind, SyncState, Version};
use rune_fuzz::Session;
use rune_tui::app::{self, App};
use rune_tui::db::DocDb;
use rune_tui::guard::GuardKind;
use rune_tui::merge::MergeState;
use rune_tui::runtime::{Effects, Msg};
use rune_tui::workspace;

use merge_common::{
    ch, deliver_op_unchecked, drain_materialize_round_trip, external_write, save_and_ack, sup,
};

/// Binds a brand-new `Document` (a second tab) onto the SAME `db_id` a
/// document already open on `path` is bound to — the shape two Explorer
/// opens of one file, or two CLI positionals resolving to one canonical
/// path, would leave behind. Joins the existing shared `FileBinding` rather
/// than reseeding it (`App::install_or_join_file_binding`'s own doc
/// comment) — a real second binding would go through the exact same call,
/// just from `db_ack::handle_load_ack`'s production path instead of this
/// test's direct construction. It stays a direct construction because every
/// user-reachable open of an already-open path (`workspace::open_path` and
/// everything funnelling through it) deduplicates to a reactivation of the
/// existing tab, so the two-tabs-one-db_id precondition is unreachable
/// through the driver's real open path.
fn bind_second_tab(
    app: &mut App,
    db_id: i64,
    path: &Path,
    content: &str,
) -> rune_tui::document::DocumentId {
    let id = app.open_document(Buffer::new(content));
    {
        let doc = app.doc_mut(id).unwrap();
        doc.file_path = Some(path.to_path_buf());
        doc.set_doc_db_for_test(DocDb::new(db_id, false, rune_db::Seq(0)));
    }
    app.install_or_join_file_binding(db_id, None);
    id
}

/// The false-conflict regression this shared baseline fixes: without it,
/// tab B's `DocDb` never learns that tab A's save advanced the file's disk
/// state, so B's very next save falsely raises the disk-conflict Guard
/// against rune's own write. Written first against the base (fails there —
/// B's stale
/// `expect_obs` still names the pre-A-save observation) and green once the
/// baseline lives on the shared `FileBinding`.
#[test]
fn bs_next_save_does_not_falsely_conflict_after_as_own_save_to_the_same_file() {
    let mut session = Session::open("/doc.md", "hello");
    let id_a = session.app().active;

    let db_id = session.app().doc(id_a).unwrap().doc_db().unwrap().db_id;
    let id_b = bind_second_tab(session.app_mut(), db_id, Path::new("/doc.md"), "hello");

    // Tab A edits and saves — a clean, ordinary CAS-matched publish.
    assert!(session.key(ch('!')).is_none());
    assert!(session.deliver_db().is_none());
    save_and_ack(&mut session);
    assert!(
        session.app().guard.is_none(),
        "test setup: tab A's own first save must not conflict"
    );
    assert_eq!(
        session.app().vfs.read(Path::new("/doc.md")).unwrap(),
        b"!hello"
    );

    // Tab B edits and saves. THE REGRESSION this pins: tab B's own `DocDb`
    // never itself witnessed tab A's save, so a per-document baseline would
    // still expect the ORIGINAL "hello" observation and CAS-refuse against
    // the "!hello" tab A just published — raising the disk-conflict Guard
    // against rune's own write.
    workspace::switch_to(session.app_mut(), id_b);
    assert!(session.deliver_db().is_none()); // the switch-triggered probe
    assert!(session.key(ch('?')).is_none());
    assert!(session.deliver_db().is_none());
    save_and_ack(&mut session);

    assert!(
        session.app().guard.is_none(),
        "tab B's save must not falsely conflict against tab A's own write to the same file"
    );
    assert_eq!(
        session.app().vfs.read(Path::new("/doc.md")).unwrap(),
        b"?hello"
    );
}

/// Mutation site (1)/(2): a force-save ("[S]ave anyway") from tab A still
/// commits through the ordinary `handle_materialize_ack` chokepoint, so it
/// must advance the SAME shared baseline tab B's own plain save reads next.
#[test]
fn force_save_from_one_tab_advances_the_shared_baseline_for_both() {
    let mut session = Session::open("/doc.md", "hello");
    let id_a = session.app().active;

    let db_id = session.app().doc(id_a).unwrap().doc_db().unwrap().db_id;
    let id_b = bind_second_tab(session.app_mut(), db_id, Path::new("/doc.md"), "hello");

    assert!(session.key(ch('!')).is_none());
    assert!(session.deliver_db().is_none());

    // An external writer moves the disk out from under tab A.
    external_write(session.app().vfs.as_ref(), b"someone else's edit");
    save_and_ack(&mut session);
    let Some(prompt) = &session.app().guard else {
        panic!("test setup: expected the disk-conflict Guard on tab A's CAS-mismatched save");
    };
    assert!(matches!(prompt.kind, GuardKind::DiskConflict));

    // "[S]ave anyway" bypasses the CAS entirely and publishes tab A's
    // buffer, advancing the SHARED baseline via the same commit chokepoint
    // an ordinary save uses.
    assert!(session.key(ch('s')).is_none());
    drain_materialize_round_trip(&mut session);
    assert!(session.app().guard.is_none());
    assert_eq!(
        session.app().vfs.read(Path::new("/doc.md")).unwrap(),
        b"!hello"
    );

    // Tab B's own plain save must now compare against the baseline tab A's
    // force-save just advanced — no conflict, even though tab B's own
    // binding never itself witnessed the force-save.
    workspace::switch_to(session.app_mut(), id_b);
    assert!(session.deliver_db().is_none());
    assert!(session.key(ch('?')).is_none());
    assert!(session.deliver_db().is_none());
    save_and_ack(&mut session);
    assert!(
        session.app().guard.is_none(),
        "tab B's save must see the baseline tab A's force-save advanced"
    );
}

/// Mutation site (3)/(4): a Discard adoption in tab A advances the CAS
/// baseline via `merge::landing::advance_expect_obs` — the shared
/// `FileBinding`, not tab A's own `DocDb`, so tab B's next save must see it
/// too.
#[test]
fn merge_discard_adoption_in_one_tab_advances_the_shared_baseline_for_both() {
    let mut session = Session::open("/doc.md", "hello");
    let id_a = session.app().active;

    let db_id = session.app().doc(id_a).unwrap().doc_db().unwrap().db_id;
    let id_b = bind_second_tab(session.app_mut(), db_id, Path::new("/doc.md"), "hello");

    assert!(session.key(ch('!')).is_none());
    assert!(session.deliver_db().is_none());

    external_write(session.app().vfs.as_ref(), b"disk changed underneath");
    save_and_ack(&mut session);
    assert!(
        session.app().guard.is_some(),
        "test setup: expected the disk-conflict Guard"
    );

    assert!(session.key(ch('d')).is_none());
    assert!(session.deliver_db_all().is_none());
    assert_eq!(session.app().merge, MergeState::Inactive);
    assert_eq!(
        session.app().doc(id_a).unwrap().buffer.content(),
        "disk changed underneath"
    );

    // Tab B's own plain save must now compare against the baseline the
    // Discard adoption just advanced for the SHARED file, not a stale
    // per-tab copy that never witnessed it.
    workspace::switch_to(session.app_mut(), id_b);
    assert!(session.deliver_db().is_none());
    assert!(session.key(ch('?')).is_none());
    assert!(session.deliver_db().is_none());
    save_and_ack(&mut session);
    assert!(
        session.app().guard.is_none(),
        "tab B's save must see the baseline tab A's Discard adoption advanced"
    );
}

/// Probe/epoch coherence: a `Probe` issued for tab B BEFORE tab A's save on
/// the SAME file, whose ack arrives after that
/// save's publish already landed, must not overwrite tab B's `last_sync`
/// with the stale classification it carries — the shared `FileBinding`'s
/// epoch bump (from ANY document's save on this `db_id`) makes the ack
/// handler drop it, exactly like the single-document case already does.
/// Out-of-order delivery goes through `merge_common::deliver_op_unchecked`,
/// since the driver's own drain is strictly oldest-first.
#[test]
fn a_stale_probe_for_tab_b_issued_before_tab_as_save_is_dropped_by_the_epoch_echo() {
    let mut session = Session::open("/doc.md", "hello");
    let id_a = session.app().active;

    let db_id = session.app().doc(id_a).unwrap().doc_db().unwrap().db_id;
    let id_b = bind_second_tab(session.app_mut(), db_id, Path::new("/doc.md"), "hello");
    session.app_mut().doc_mut(id_b).unwrap().last_sync = Some(SyncKind::Clean);

    // Switching onto tab B issues its own probe — leave its ack outstanding.
    workspace::switch_to(session.app_mut(), id_b);
    let probe_op = *session
        .app()
        .db_ops
        .iter()
        .find(|(_, pending)| pending.doc == id_b && pending.is_probe)
        .expect("probe enqueued for tab b")
        .0;

    // Switch back to tab A and drive a real save all the way to its own
    // record ack, delivering every op EXCEPT tab B's still-outstanding probe.
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
        .find(|id| **id != probe_op)
        .expect("append-edit op enqueued");
    deliver_op_unchecked(&mut session, edit_op);

    assert!(session.key(sup('s')).is_none());
    let prepare_op = *session
        .app()
        .db_ops
        .keys()
        .find(|id| **id != probe_op)
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
        .find(|id| **id != probe_op)
        .expect("materialize-record op enqueued");
    deliver_op_unchecked(&mut session, record_op);

    assert_eq!(
        session.app().file_binding(db_id).unwrap().baseline_epoch,
        1,
        "test setup: tab A's committed save must have bumped the SHARED save epoch"
    );

    // Feed tab B's pre-save probe's ack now, carrying an obviously wrong
    // classification — if it were ever applied it would be visible.
    let stale = SyncState {
        kind: SyncKind::Diverged,
        ancestor: None,
        ours: Version {
            hash: rune_db::BlobHash("ours".to_string()),
            obs: None,
        },
        theirs: None,
    };
    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Db(DbEvent::Ok {
            id: probe_op,
            result: OpOutcome::Sync(Box::new(stale)),
        }),
        &mut effects,
    );

    assert_eq!(
        session.app().doc(id_b).unwrap().last_sync,
        Some(SyncKind::Clean),
        "a probe issued for tab B before tab A's save on the SAME file must not overwrite \
         last_sync with a stale classification once the SHARED epoch has advanced"
    );
}
