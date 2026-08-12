//! Blockers 1 and 2 (second-review fixes on `wt-rec-tui`): a `Materialize
//! Record` op that already ENQUEUED successfully (the writer was alive at
//! that instant) can still see its eventual reply arrive as `DbEvent::Err`
//! (or `Fatal`) if the writer thread dies before processing it —
//! `db_dispatch::handle_db_event`'s `Err`/`Fatal` arms deliberately call
//! `handle_materialize_ack(committed: true, ..Default)` FIRST, before
//! `on_store_failure` marks the store degraded, so a write that already
//! physically committed is never reported as a failed save. At the instant
//! that synthetic ack runs, the store is therefore still live/non-degraded
//! from `handle_materialize_ack`'s own point of view — exactly the window
//! its `saved: None` re-baseline arm reads as "the store can still serve a
//! `Load`".
//!
//! Simulated deterministically with `Store::kill_writer_for_test` +
//! `probe_blocking_for_test` (a real writer-death confirmation, no wall-
//! clock wait) to make the re-baseline's own `Load` enqueue genuinely fail,
//! then a hand-built `DbEvent::Err` (the shape a real SQL failure on an
//! already-enqueued op would deliver) stands in for that failure — `rune-db`
//! has no black-box hook to force one from outside, but the app-side
//! reaction under test only ever looks at the `DbEvent`/`App` shape, never
//! at what produced it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod rename_common;

use std::path::Path;

use rune_db::DbEvent;
use rune_tui::runtime::{CmdKind, Msg};
use rune_vfs::Vfs;

use rename_common::{seeded_vfs, send, sup, type_text};

/// Blocks until `store`'s writer thread is CONFIRMED gone (never a fixed
/// spin count or a wall-clock sleep) — mirrors `db_wiring_degraded.rs`'s
/// own `a_dead_writer_thread_still_lets_the_save_reach_disk` fixture.
fn wait_for_writer_death(store: &rune_db::Store, doc_id: i64) {
    let max_attempts = 4 * rune_db::QUEUE_DEPTH;
    for attempt in 0..=max_attempts {
        match store.probe_blocking_for_test(rune_db::DocId(doc_id)) {
            Ok(_) => assert!(
                attempt < max_attempts,
                "writer never confirmed dead after {max_attempts} blocking probes"
            ),
            Err(rune_db::Error::WriterGone) => return,
            Err(e) => panic!("unexpected error while awaiting writer death: {e}"),
        }
    }
}

/// B1: without resolving the save FIRST, the re-baseline `Load` this
/// synthetic commit enqueues can fail (writer gone) and sweep `id`'s own
/// still-pending `save_pending` out from under it before `finish_save_ok`
/// ever runs — a write that ALREADY landed on disk then reads dirty
/// forever. B2: the binding that `Load` failed to refresh must also be
/// dropped, not left standing with a stale `expect_obs` no future
/// `materialize_prepare` will ever find.
#[test]
fn a_dead_writer_racing_its_own_materialize_record_ack_still_resolves_the_save_and_drops_the_binding()
 {
    let mem = seeded_vfs();
    let (mut app, bridge) = rename_common::app_with_store(&mem);
    let id = app.active;

    // Dirty the buffer while the writer is still healthy, then save.
    type_text(&mut app, "!");
    send(&mut app, sup('s'));

    let prep_evt = rename_common::wait_for_materialize_prep(&bridge);
    let mut effects = send(&mut app, Msg::Db(prep_evt));
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Save)
        .expect("the prepare ack must spawn the caller-side vfs Cmd");
    let vfs_done = cmd.run().expect("the vfs Cmd must reply");
    // The write physically lands here, synchronously, through this app's
    // own `Vfs` — entirely independent of the writer thread's fate.
    send(&mut app, vfs_done);

    // The `MaterializeRecord` enqueue above succeeded (the writer was still
    // alive) — the document is now `Recording { published: true }` (the
    // disk write already committed).
    assert_eq!(
        app.doc(id).unwrap().save_phase(),
        rune_tui::document::SavePhase::Recording { published: true }
    );
    let op_id = app
        .doc(id)
        .unwrap()
        .record_op()
        .expect("the committed write must have enqueued a MaterializeRecord");

    // The writer dies BEFORE that op's own reply would have arrived —
    // deterministically confirmed, never raced.
    let db_id = app.doc(id).unwrap().doc_db().unwrap().db_id;
    {
        let store = &app.db.as_ref().unwrap().store;
        store.kill_writer_for_test().expect("enqueue the kill op");
        wait_for_writer_death(store, db_id);
    }
    assert!(
        !app.db.as_ref().unwrap().degraded,
        "test setup: the store must still read non-degraded going into the Err ack, \
         exactly like the real db_dispatch race this reproduces"
    );

    // Stands in for the writer failing to process the already-enqueued op
    // (a real SQL/I/O failure) — `db_dispatch::handle_db_event`'s `Err` arm
    // is what actually matters here, not what produced the `DbEvent`.
    send(
        &mut app,
        Msg::Db(DbEvent::Err {
            id: op_id,
            error: "simulated write failure".to_string(),
        }),
    );

    assert_eq!(
        mem.read(Path::new("/root/a.md")).expect("file present"),
        b"!a content",
        "the write itself already committed before the writer died"
    );
    assert!(
        !app.doc(id).unwrap().is_dirty(),
        "B1: a save whose bytes already landed on disk must never read as \
         dirty just because its own re-baseline Load lost a race with a dead writer"
    );
    assert!(
        !app.doc(id).unwrap().is_store_bound(),
        "B2: a binding the re-baseline Load could not refresh must be dropped, \
         never left standing with a stale expect_obs"
    );
    assert!(
        app.db.as_ref().unwrap().degraded,
        "the confirmed-dead writer must still degrade the store"
    );
}
