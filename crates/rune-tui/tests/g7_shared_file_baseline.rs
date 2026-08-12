//! Integration tests for the shared save-CAS baseline: `expect_obs`/
//! `pending_rebaseline_hash`/`save_epoch` are shared per `db_id`
//! (`App::file_bindings`), not copied per `Document` — two `Document`s
//! (tabs) bound to the SAME underlying file must see the one truth about
//! what disk holds. Follows the `merge_common`/`merge_disk_conflict_guard.
//! rs`/`probe_save_epoch.rs` fixture patterns, constructing a second
//! `Document` bound to the SAME `db_id` the same way `materialize_fatal_
//! two_docs.rs` does.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod merge_common;

use std::path::Path;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_db::{DbEvent, OpOutcome, SyncKind, SyncState, Version};
use rune_tui::app::{self, App};
use rune_tui::db::{DbBridge, DocDb};
use rune_tui::guard::GuardKind;
use rune_tui::merge::MergeState;
use rune_tui::runtime::{CmdKind, Effects, Msg};
use rune_tui::workspace;
use rune_vfs::{Mem, Vfs};

use merge_common::db_wiring_common::{app_with_store, publish, recv_ok};
use merge_common::{
    ch, drain_all_ops_for, drain_one_op_for, external_write, press_key, save_and_ack, sup,
};

/// Binds a brand-new `Document` (a second tab) onto the SAME `db_id` a
/// document already open on `path` is bound to — the shape two Explorer
/// opens of one file, or two CLI positionals resolving to one canonical
/// path, would leave behind. Joins the existing shared `FileBinding` rather
/// than reseeding it (`App::install_or_join_file_binding`'s own doc comment) — a real second
/// binding would go through the exact same call, just from `db_ack::
/// handle_load_ack`'s production path instead of this test's direct
/// construction.
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
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, bridge) = app_with_store("g7-two-tabs-save", Arc::clone(&vfs));
    workspace::open_path(&mut app, Path::new("/doc.md"));
    let id_a = app.active;
    drain_one_op_for(&mut app, &bridge, id_a);

    let db_id = app.doc(id_a).unwrap().doc_db().unwrap().db_id;
    let id_b = bind_second_tab(&mut app, db_id, Path::new("/doc.md"), "hello");

    // Tab A edits and saves — a clean, ordinary CAS-matched publish.
    press_key(&mut app, ch('!'));
    drain_one_op_for(&mut app, &bridge, id_a);
    save_and_ack(&mut app, &bridge, id_a);
    assert!(
        app.guard.is_none(),
        "test setup: tab A's own first save must not conflict"
    );
    assert_eq!(vfs.read(Path::new("/doc.md")).unwrap(), b"!hello");

    // Tab B edits and saves. THE REGRESSION this pins: tab B's own `DocDb`
    // never itself witnessed tab A's save, so a per-document baseline would
    // still expect the ORIGINAL "hello" observation and CAS-refuse against
    // the "!hello" tab A just published — raising the disk-conflict Guard
    // against rune's own write.
    workspace::switch_to(&mut app, id_b);
    drain_one_op_for(&mut app, &bridge, id_b); // the switch-triggered probe
    press_key(&mut app, ch('?'));
    drain_one_op_for(&mut app, &bridge, id_b);
    save_and_ack(&mut app, &bridge, id_b);

    assert!(
        app.guard.is_none(),
        "tab B's save must not falsely conflict against tab A's own write to the same file"
    );
    assert_eq!(vfs.read(Path::new("/doc.md")).unwrap(), b"?hello");
}

/// Mutation site (1)/(2): a force-save ("[S]ave anyway") from tab A still
/// commits through the ordinary `handle_materialize_ack` chokepoint, so it
/// must advance the SAME shared baseline tab B's own plain save reads next.
#[test]
fn force_save_from_one_tab_advances_the_shared_baseline_for_both() {
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, bridge) = app_with_store("g7-force-save", Arc::clone(&vfs));
    workspace::open_path(&mut app, Path::new("/doc.md"));
    let id_a = app.active;
    drain_one_op_for(&mut app, &bridge, id_a);

    let db_id = app.doc(id_a).unwrap().doc_db().unwrap().db_id;
    let id_b = bind_second_tab(&mut app, db_id, Path::new("/doc.md"), "hello");

    press_key(&mut app, ch('!'));
    drain_one_op_for(&mut app, &bridge, id_a);

    // An external writer moves the disk out from under tab A.
    external_write(vfs.as_ref(), b"someone else's edit");
    save_and_ack(&mut app, &bridge, id_a);
    let Some(prompt) = &app.guard else {
        panic!("test setup: expected the disk-conflict Guard on tab A's CAS-mismatched save");
    };
    assert!(matches!(prompt.kind, GuardKind::DiskConflict));

    // "[S]ave anyway" bypasses the CAS entirely and publishes tab A's
    // buffer, advancing the SHARED baseline via the same commit chokepoint
    // an ordinary save uses.
    press_key(&mut app, ch('s'));
    merge_common::drain_materialize_round_trip(&mut app, &bridge, id_a);
    assert!(app.guard.is_none());
    assert_eq!(vfs.read(Path::new("/doc.md")).unwrap(), b"!hello");

    // Tab B's own plain save must now compare against the baseline tab A's
    // force-save just advanced — no conflict, even though tab B's own
    // binding never itself witnessed the force-save.
    workspace::switch_to(&mut app, id_b);
    drain_one_op_for(&mut app, &bridge, id_b);
    press_key(&mut app, ch('?'));
    drain_one_op_for(&mut app, &bridge, id_b);
    save_and_ack(&mut app, &bridge, id_b);
    assert!(
        app.guard.is_none(),
        "tab B's save must see the baseline tab A's force-save advanced"
    );
}

/// Mutation site (3)/(4): a Discard adoption in tab A advances the CAS
/// baseline via `merge::landing::advance_expect_obs` — the shared
/// `FileBinding`, not tab A's own `DocDb`, so tab B's next save must see it
/// too.
#[test]
fn merge_discard_adoption_in_one_tab_advances_the_shared_baseline_for_both() {
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, bridge) = app_with_store("g7-merge-discard", Arc::clone(&vfs));
    workspace::open_path(&mut app, Path::new("/doc.md"));
    let id_a = app.active;
    drain_one_op_for(&mut app, &bridge, id_a);

    let db_id = app.doc(id_a).unwrap().doc_db().unwrap().db_id;
    let id_b = bind_second_tab(&mut app, db_id, Path::new("/doc.md"), "hello");

    press_key(&mut app, ch('!'));
    drain_one_op_for(&mut app, &bridge, id_a);

    external_write(vfs.as_ref(), b"disk changed underneath");
    save_and_ack(&mut app, &bridge, id_a);
    assert!(
        app.guard.is_some(),
        "test setup: expected the disk-conflict Guard"
    );

    press_key(&mut app, ch('d'));
    drain_all_ops_for(&mut app, &bridge, id_a);
    assert_eq!(app.merge, MergeState::Inactive);
    assert_eq!(
        app.doc(id_a).unwrap().buffer.content(),
        "disk changed underneath"
    );

    // Tab B's own plain save must now compare against the baseline the
    // Discard adoption just advanced for the SHARED file, not a stale
    // per-tab copy that never witnessed it.
    workspace::switch_to(&mut app, id_b);
    drain_one_op_for(&mut app, &bridge, id_b);
    press_key(&mut app, ch('?'));
    drain_one_op_for(&mut app, &bridge, id_b);
    save_and_ack(&mut app, &bridge, id_b);
    assert!(
        app.guard.is_none(),
        "tab B's save must see the baseline tab A's Discard adoption advanced"
    );
}

/// Delivers exactly the op named by `op_id` — needed here because these
/// tests deliberately leave tab B's own `Probe` outstanding alongside tab
/// A's edit/save ops, to control which one lands first (mirrors `probe_
/// save_epoch.rs`'s own `drain_specific`, private to that file).
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

/// Probe/epoch coherence: a `Probe` issued for tab B BEFORE tab A's save on
/// the SAME file, whose ack arrives after that
/// save's publish already landed, must not overwrite tab B's `last_sync`
/// with the stale classification it carries — the shared `FileBinding`'s
/// epoch bump (from ANY document's save on this `db_id`) makes the ack
/// handler drop it, exactly like the single-document case already does.
#[test]
fn a_stale_probe_for_tab_b_issued_before_tab_as_save_is_dropped_by_the_epoch_echo() {
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, bridge) = app_with_store("g7-probe-epoch", Arc::clone(&vfs));
    workspace::open_path(&mut app, Path::new("/doc.md"));
    let id_a = app.active;
    drain_one_op_for(&mut app, &bridge, id_a);

    let db_id = app.doc(id_a).unwrap().doc_db().unwrap().db_id;
    let id_b = bind_second_tab(&mut app, db_id, Path::new("/doc.md"), "hello");
    app.doc_mut(id_b).unwrap().last_sync = Some(SyncKind::Clean);

    // Switching onto tab B issues its own probe — leave its ack outstanding.
    workspace::switch_to(&mut app, id_b);
    let probe_op = *app
        .db_ops
        .iter()
        .find(|(_, pending)| pending.doc == id_b && pending.is_probe)
        .expect("probe enqueued for tab b")
        .0;

    // Switch back to tab A and drive a real save all the way to its own
    // record ack, draining every op EXCEPT tab B's still-outstanding probe.
    workspace::switch_to(&mut app, id_a);
    let a_probe = *app
        .db_ops
        .iter()
        .find(|(_, pending)| pending.doc == id_a && pending.is_probe)
        .expect("probe enqueued for tab a")
        .0;
    drain_specific(&mut app, &bridge, a_probe);

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
        .find(|c| c.kind() == CmdKind::Save)
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

    assert_eq!(
        app.file_binding(db_id).unwrap().save_epoch,
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
        &mut app,
        Msg::Db(DbEvent::Ok {
            id: probe_op,
            result: OpOutcome::Sync(Box::new(stale)),
        }),
        &mut effects,
    );

    assert_eq!(
        app.doc(id_b).unwrap().last_sync,
        Some(SyncKind::Clean),
        "a probe issued for tab B before tab A's save on the SAME file must not overwrite \
         last_sync with a stale classification once the SHARED epoch has advanced"
    );
}
