//! Tests for the save-epoch echo on `Probe` acks and the save-in-flight
//! probe deferral — structural echo suppression, so a stale `Probe` reply
//! can never overwrite what a later save already made true. Follows the
//! `db_wiring_sync.rs`/`merge_disk_conflict_guard.rs` pattern, pulling
//! shared fixtures from `merge_common`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod merge_common;

use std::path::Path;
use std::sync::Arc;

use rune_db::{DbEvent, OpOutcome, SyncKind, SyncState, Version};
use rune_tui::app::{self, App};
use rune_tui::db::DbBridge;
use rune_tui::runtime::{Effects, Msg};
use rune_tui::workspace;
use rune_vfs::{Mem, Vfs};

use merge_common::db_wiring_common::{app_with_store, publish, recv_ok};
use merge_common::{
    ch, drain_materialize_round_trip, drain_one_op_for, external_write, press_key, sup,
};

fn fake_version(hash: &str) -> Version {
    Version {
        hash: hash.to_string(),
        obs: None,
    }
}

/// Drains exactly the op named by `op_id`, unlike `merge_common::drain_one_
/// op_for` (which picks whichever single op is recorded for a document and
/// panics if more than one is outstanding) — needed here because these
/// tests deliberately leave a `Probe` outstanding alongside the save's own
/// ops, to control which one lands first.
fn drain_specific(app: &mut App, bridge: &DbBridge, op_id: u64) -> Effects {
    let result = recv_ok(bridge, op_id);
    let mut effects = Effects::default();
    app::update(
        app,
        Msg::Db(DbEvent::Ok { id: op_id, result }),
        &mut effects,
    );
    effects
}

/// A `Probe` issued before a save's publish, whose ack
/// arrives after the publish already landed, must not overwrite
/// `last_sync` with the classification it carried — the save's own epoch
/// bump makes the ack handler drop it, mirroring the merge-generation
/// ticket check.
#[test]
fn stale_pre_save_probe_ack_never_overwrites_post_save_last_sync() {
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, bridge) = app_with_store("probe-epoch-stale", Arc::clone(&vfs));
    let draft_id = app.active;

    workspace::open_path(&mut app, Path::new("/doc.md"));
    let doc_id = app.active;
    drain_one_op_for(&mut app, &bridge, doc_id);
    assert_eq!(app.doc(doc_id).unwrap().last_sync, Some(SyncKind::Clean));

    // Issue a probe now, but leave its ack outstanding — the save below
    // lands before this one ever gets drained.
    workspace::switch_to(&mut app, draft_id);
    workspace::switch_to(&mut app, doc_id);
    let probe_op = *app
        .db_ops
        .iter()
        .find(|(_, pending)| pending.doc == doc_id && pending.is_probe)
        .expect("probe enqueued")
        .0;

    // Edit and drive a real save all the way to its own record ack,
    // draining every op EXCEPT the still-outstanding probe above.
    press_key(&mut app, ch('!'));
    let edit_op = *app
        .db_ops
        .keys()
        .find(|id| **id != probe_op)
        .expect("append-edit op enqueued");
    drain_specific(&mut app, &bridge, edit_op);

    press_key(&mut app, sup('s'));
    let prepare_op = *app
        .db_ops
        .keys()
        .find(|id| **id != probe_op)
        .expect("materialize-prepare op enqueued");
    let prepare_effects = drain_specific(&mut app, &bridge, prepare_op);
    let save_cmd = prepare_effects
        .cmds
        .into_iter()
        .find(|c| c.kind() == rune_tui::runtime::CmdKind::Save)
        .expect("the prepare ack must spawn the caller-side vfs Cmd");
    let vfs_done_msg = save_cmd.run().expect("the vfs Cmd must reply");
    let mut effects = Effects::default();
    app::update(&mut app, vfs_done_msg, &mut effects);
    let record_op = *app
        .db_ops
        .keys()
        .find(|id| **id != probe_op)
        .expect("materialize-record op enqueued");
    drain_specific(&mut app, &bridge, record_op);

    let db_id = app.doc(doc_id).unwrap().db.as_ref().unwrap().db_id;
    assert_eq!(
        app.file_binding(db_id).unwrap().save_epoch,
        1,
        "test setup: the committed save must have bumped the save epoch"
    );
    assert_eq!(
        app.doc(doc_id).unwrap().last_sync,
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
        &mut app,
        Msg::Db(DbEvent::Ok {
            id: probe_op,
            result: OpOutcome::Sync(Box::new(stale)),
        }),
        &mut effects,
    );

    assert_eq!(
        app.doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Clean),
        "a probe issued before the save's publish must not overwrite last_sync \
         with a stale classification once the epoch has advanced"
    );
    assert!(
        !app.db_ops.contains_key(&probe_op),
        "the stale probe op id must not linger in db_ops"
    );
    let reissued = app
        .db_ops
        .values()
        .find(|pending| pending.doc == doc_id && pending.is_probe)
        .expect("a dropped stale ack must re-issue a fresh probe, not leave last_sync stale");
    assert_eq!(
        reissued.probe_epoch,
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
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, bridge) = app_with_store("probe-epoch-deferred", Arc::clone(&vfs));
    let draft_id = app.active;

    workspace::open_path(&mut app, Path::new("/doc.md"));
    let doc_id = app.active;
    drain_one_op_for(&mut app, &bridge, doc_id);

    press_key(&mut app, ch('!'));
    drain_one_op_for(&mut app, &bridge, doc_id);

    press_key(&mut app, sup('s'));
    assert!(
        app.doc(doc_id).unwrap().save_in_flight(),
        "test setup: the save must be in flight"
    );

    // A tab switch while the save is still in flight must defer, not
    // enqueue, the probe it would otherwise issue.
    workspace::switch_to(&mut app, draft_id);
    workspace::switch_to(&mut app, doc_id);
    assert_eq!(
        app.db_ops.len(),
        1,
        "the probe must be deferred, not enqueued, while a save is in flight"
    );
    let db_id = app.doc(doc_id).unwrap().db.as_ref().unwrap().db_id;
    assert!(
        app.file_binding(db_id).unwrap().pending_probe,
        "the deferred probe request must be recorded on the shared FileBinding"
    );

    drain_materialize_round_trip(&mut app, &bridge, doc_id);

    assert!(
        !app.file_binding(db_id).unwrap().pending_probe,
        "the deferral flag must be consumed once the save resolves"
    );
    assert!(
        app.db_ops.is_empty(),
        "the deferred probe's own ack must be fully drained too, not left outstanding"
    );
    assert_eq!(
        app.doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Clean),
        "the deferred probe must read the post-save disk, which matches the just-saved buffer"
    );
}

/// A probe with no save intervening at all must classify
/// exactly as before this change — the epoch/deferral machinery introduced
/// here must not alter the ordinary, no-race path.
#[test]
fn probe_without_an_intervening_save_still_classifies_normally() {
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, bridge) = app_with_store("probe-epoch-unchanged", Arc::clone(&vfs));
    let draft_id = app.active;

    workspace::open_path(&mut app, Path::new("/doc.md"));
    let doc_id = app.active;
    drain_one_op_for(&mut app, &bridge, doc_id);
    assert_eq!(app.doc(doc_id).unwrap().last_sync, Some(SyncKind::Clean));
    let db_id = app.doc(doc_id).unwrap().db.as_ref().unwrap().db_id;
    assert_eq!(
        app.file_binding(db_id).unwrap().save_epoch,
        0,
        "test setup: no save has happened yet"
    );

    external_write(vfs.as_ref(), b"changed externally");

    workspace::switch_to(&mut app, draft_id);
    workspace::switch_to(&mut app, doc_id);
    drain_one_op_for(&mut app, &bridge, doc_id);

    assert_eq!(
        app.doc(doc_id).unwrap().last_sync,
        Some(SyncKind::DiskAhead),
        "an ordinary probe with no save in flight must classify exactly as \
         it did before the epoch/deferral machinery existed"
    );
}
