//! Tests for the save-epoch echo on `Probe` acks and the save-in-flight
//! probe deferral — structural echo suppression, so a stale `Probe` reply
//! can never overwrite what a later save already made true. Driven through
//! `rune_fuzz::Session`, pulling shared fixtures from `merge_common`;
//! out-of-order ack delivery (a probe deliberately held back) goes through
//! `merge_common::deliver_op_unchecked`, since the driver's own drain is
//! strictly oldest-first.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod merge_common;

use rune_db::{DbEvent, OpOutcome, SyncKind, SyncState, Version};
use rune_fuzz::Session;
use rune_tui::app;
use rune_tui::runtime::{CmdKind, Effects, Msg};
use rune_tui::workspace;

use merge_common::{
    ch, deliver_op_unchecked, drain_materialize_round_trip, external_write, reprobe, sup,
    untitled_draft,
};

fn fake_version(hash: &str) -> Version {
    Version {
        hash: rune_db::BlobHash(hash.to_string()),
        obs: None,
    }
}

/// A `Probe` issued before a save's publish, whose ack
/// arrives after the publish already landed, must not overwrite
/// `last_sync` with the classification it carried — the save's own epoch
/// bump makes the ack handler drop it, mirroring the merge-generation
/// ticket check.
#[test]
fn stale_pre_save_probe_ack_never_overwrites_post_save_last_sync() {
    let mut session = Session::open("/doc.md", "hello");
    let doc_id = session.app().active;
    let draft_id = untitled_draft(session.app(), doc_id);
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Clean)
    );

    // Issue a probe now, but leave its ack outstanding — the save below
    // lands before this one ever gets delivered.
    workspace::switch_to(session.app_mut(), draft_id);
    workspace::switch_to(session.app_mut(), doc_id);
    let probe_op = *session
        .app()
        .db_ops
        .iter()
        .find(|(_, pending)| pending.doc == doc_id && pending.is_probe)
        .expect("probe enqueued")
        .0;

    // Edit and drive a real save all the way to its own record ack,
    // delivering every op EXCEPT the still-outstanding probe above.
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
        .find(|c| c.kind() == CmdKind::Save)
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

    let db_id = session.app().doc(doc_id).unwrap().doc_db().unwrap().db_id;
    assert_eq!(
        session.app().file_binding(db_id).unwrap().baseline_epoch,
        1,
        "test setup: the committed save must have bumped the save epoch"
    );
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Clean),
        "test setup: the successful save must not have touched last_sync itself"
    );

    // Feed the pre-save probe's ack now, carrying an obviously wrong
    // classification — if it were ever applied it would be visible.
    let stale = SyncState {
        kind: SyncKind::Diverged,
        ancestor: None,
        ours: fake_version("ours"),
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
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Clean),
        "a probe issued before the save's publish must not overwrite last_sync \
         with a stale classification once the epoch has advanced"
    );
    assert!(
        !session.app().db_ops.contains_key(&probe_op),
        "the stale probe op id must not linger in db_ops"
    );
    let reissued = session
        .app()
        .db_ops
        .values()
        .find(|pending| pending.doc == doc_id && pending.is_probe)
        .expect("a dropped stale ack must re-issue a fresh probe, not leave last_sync stale");
    assert_eq!(
        reissued.baseline_epoch,
        Some(1),
        "the re-issued probe must record the CURRENT save epoch"
    );
}

/// A tab switch while a save is in flight must defer the
/// probe it would otherwise enqueue, and fire it exactly once the save's
/// ack resolves — ending with a disk fact read fresh from the post-save
/// world.
#[test]
fn probe_deferred_during_save_in_flight_fires_after_the_ack_with_correct_sync_state() {
    let mut session = Session::open("/doc.md", "hello");
    let doc_id = session.app().active;
    let draft_id = untitled_draft(session.app(), doc_id);

    assert!(session.key(ch('!')).is_none());
    assert!(session.deliver_db().is_none());

    assert!(session.key(sup('s')).is_none());
    assert!(
        session.app().doc(doc_id).unwrap().save_in_flight(),
        "test setup: the save must be in flight"
    );

    // A tab switch while the save is still in flight must defer, not
    // enqueue, the probe it would otherwise issue.
    workspace::switch_to(session.app_mut(), draft_id);
    workspace::switch_to(session.app_mut(), doc_id);
    assert_eq!(
        session.app().db_ops.len(),
        1,
        "the probe must be deferred, not enqueued, while a save is in flight"
    );
    let db_id = session.app().doc(doc_id).unwrap().doc_db().unwrap().db_id;
    assert!(
        session.app().file_binding(db_id).unwrap().pending_probe,
        "the deferred probe request must be recorded on the shared FileBinding"
    );

    drain_materialize_round_trip(&mut session);

    assert!(
        !session.app().file_binding(db_id).unwrap().pending_probe,
        "the deferral flag must be consumed once the save resolves"
    );
    assert!(
        session.app().db_ops.is_empty(),
        "the deferred probe's own ack must be fully delivered too, not left outstanding"
    );
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Clean),
        "the deferred probe must read the post-save disk, which matches the just-saved buffer"
    );
}

/// A probe with no save intervening at all must classify
/// exactly as before this change — the epoch/deferral machinery introduced
/// here must not alter the ordinary, no-race path.
#[test]
fn probe_without_an_intervening_save_still_classifies_normally() {
    let mut session = Session::open("/doc.md", "hello");
    let doc_id = session.app().active;
    let draft_id = untitled_draft(session.app(), doc_id);
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Clean)
    );
    let db_id = session.app().doc(doc_id).unwrap().doc_db().unwrap().db_id;
    assert_eq!(
        session.app().file_binding(db_id).unwrap().baseline_epoch,
        0,
        "test setup: no save has happened yet"
    );

    external_write(session.app().vfs.as_ref(), b"changed externally");

    reprobe(&mut session, draft_id, doc_id);

    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::DiskAhead),
        "an ordinary probe with no save in flight must classify exactly as \
         it did before the epoch/deferral machinery existed"
    );
}
