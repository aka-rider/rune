//! WP5 "Done when" integration tests for the rune-tui <-> rune-db wiring's
//! degraded-store banner (plan decision 3) and its `super+s` confirm gate
//! (plan WP5.S2/S6) — TODO.md's 500-line budget split of the original `db_wiring.rs`.
//! Restart/hydration and open/close lifecycle tests live in the sibling
//! `db_wiring_hydrate.rs`/`db_wiring_lifecycle.rs`; all three pull shared
//! fixtures from `db_wiring_common`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod db_wiring_common;
mod dirty_common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_db::{ClockFn, Store};
use rune_tui::app::{self, App};
use rune_tui::db::{Db, DocDb};
use rune_tui::footer;
use rune_tui::runtime::Effects;
use rune_tui::runtime::Msg;
use rune_vfs::{Mem, Vfs};

use db_wiring_common::{
    db_from, doc_db_from, open_and_load, press, publish, save_key, temp_db_dir,
};

/// Plan WP5 "Done when": type -> kill the store writer via the test hook
/// (`Store::kill_writer_for_test`) -> the persistent degraded banner
/// appears in `footer::footer_text`'s output, and the buffer's content is
/// NEVER rolled back (plan decision 3: an enqueue-time failure only
/// degrades the store — it never touches the in-memory buffer/journal).
#[test]
fn killed_writer_surfaces_a_degraded_banner_without_rolling_back_the_buffer() {
    let dir = temp_db_dir("kill-writer");
    let db_path = dir.join("rune-v1.db");
    let doc_path = Path::new("/doc.md");

    let mem = Mem::new();
    publish(&mem, doc_path, b"hi");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (store, bridge, load) = open_and_load(&db_path, Arc::clone(&vfs), doc_path);
    assert!(
        !store.degraded(),
        "a fresh temp-dir store must not open degraded"
    );
    let db = db_from(store, bridge, false);
    let doc_db = doc_db_from(&load);

    let mut app = App::new(
        Buffer::new(load.recovered.clone()),
        Some(doc_path.to_path_buf()),
        vfs,
        Some(db),
    );
    let id = app.active;
    app.doc_mut(id).unwrap().db = Some(doc_db);
    let len = app.doc(id).unwrap().buffer.len();
    app.doc_mut(id).unwrap().cursors = CursorSet::new(len);

    // Typing while the store is healthy journals normally — no banner.
    press(&mut app, '!');
    assert_eq!(app.doc(id).unwrap().buffer.content(), "hi!");
    assert!(app.db_banner.is_none());

    app.db
        .as_ref()
        .expect("app has a store")
        .store
        .kill_writer_for_test()
        .expect("enqueue the kill op");

    // Keep typing until the (now-dying) writer's enqueue failure surfaces
    // the banner. Bounded spin, not a wall-clock sleep (repo convention):
    // the kill op must first be DEQUEUED by the writer thread before it
    // takes effect, so exactly how many further enqueues still succeed is
    // a genuine race, not something a fixed count can predict up front.
    let mut typed = String::from("hi!");
    let mut saw_banner = false;
    for i in 0..2000 {
        let ch = if i % 2 == 0 { 'a' } else { 'b' };
        press(&mut app, ch);
        typed.push(ch);
        if app.db_banner.is_some() {
            saw_banner = true;
            break;
        }
    }

    assert!(
        saw_banner,
        "the degraded banner must appear once the writer is confirmed gone"
    );
    assert!(
        app.db_banner
            .as_deref()
            .is_some_and(|b| b.contains("recovery disabled")),
        "banner text must read 'recovery disabled: <err>' (got {:?})",
        app.db_banner
    );
    assert!(
        footer::footer_text(&app).contains("recovery disabled"),
        "the banner must be part of the rendered footer line"
    );
    assert!(
        app.db.as_ref().is_some_and(|d| d.degraded),
        "the store must be marked degraded"
    );
    // No rollback, ever (plan decision 3): the buffer must reflect EVERY
    // keystroke typed so far, regardless of exactly when the writer died
    // relative to these presses.
    assert_eq!(
        app.doc(id).unwrap().buffer.content(),
        typed,
        "a store failure must never roll back the in-memory buffer"
    );
}

/// Finding 5 / [rune-db 1] (WP7): a `MaterializePrepare` enqueue failure
/// (the store writer confirmed gone) must degrade the store and raise the
/// sticky banner through the SAME `on_store_failure` chokepoint
/// `append_edit`/`move_undo_pos` use — never a one-shot `SaveError` status
/// that leaves `db.degraded` untouched. Distinct from the pre-WP7 bug this
/// finding described: a dead writer must not ALSO make the save itself
/// impossible — WP7's `materialize_now` falls back to the same
/// uncoordinated direct-`vfs` `Cmd` a document with no store binding uses,
/// so `save_in_flight` stays true (a real write is now in flight) and,
/// once that `Cmd` runs, the user's bytes are actually on disk — "press
/// ⌘S again to save anyway" must actually save. Deterministically waits
/// for the writer to be CONFIRMED gone via a BLOCKING probe send that is
/// woken with `Err(WriterGone)` only when the writer drops its queue
/// receiver — a full queue merely parks the wait and never counts as
/// confirmation — before pressing save exactly once, rather than racing
/// `super+s`'s own in-flight latch against the kill op's async dequeue.
#[test]
fn a_dead_writer_thread_still_lets_the_save_reach_disk() {
    let dir = temp_db_dir("kill-writer-materialize");
    let db_path = dir.join("rune-v1.db");
    let doc_path = Path::new("/doc.md");

    let mem = Mem::new();
    publish(&mem, doc_path, b"hi");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (store, bridge, load) = open_and_load(&db_path, Arc::clone(&vfs), doc_path);
    let db = db_from(store, bridge, false);
    let doc_db = doc_db_from(&load);

    let mut app = App::new(
        Buffer::new(load.recovered.clone()),
        Some(doc_path.to_path_buf()),
        Arc::clone(&vfs),
        Some(db),
    );
    let id = app.active;
    app.doc_mut(id).unwrap().db = Some(doc_db);
    let len = app.doc(id).unwrap().buffer.len();
    app.doc_mut(id).unwrap().cursors = CursorSet::new(len);

    // Dirty the buffer (a healthy edit — the writer is still alive here) so
    // `trigger_save` below actually has something to save.
    press(&mut app, '!');
    assert!(app.db_banner.is_none());

    let db = app.db.as_ref().expect("app has a store");
    db.store
        .kill_writer_for_test()
        .expect("enqueue the kill op");

    // Condition-driven wait, not a spin and not a wall-clock sleep: the
    // kill op only takes effect once the writer thread DEQUEUES it, and
    // the one signal that has happened is the writer dropping its side of
    // the queue — which is exactly what wakes a blocking send. Each `Ok`
    // means the probe was accepted (a live writer drained a slot, or a
    // freed slot absorbed it); the writer is FIFO-bound to reach the kill
    // op after finitely many of those, so this loop terminates by writer
    // progress alone. `WriterQueueFull` must NEVER count as confirmation:
    // a full queue with a live writer can free a slot for the very next
    // enqueue (the super+s below), which would then succeed and never
    // trip `on_store_failure` — the exact captured flake this replaces.
    // With a blocking send that ambiguity is unobservable by construction:
    // there is no full-queue error case, only writer-death.
    //
    // Bounded, not an unbounded spin: a blocking send returns `Ok` only
    // when the writer consumed a slot or a slot was free, so a live writer
    // FIFO-bound to the kill op can absorb at most (ops queued ahead of
    // the kill) + `QUEUE_DEPTH` probes before it must have dequeued the
    // kill op and dropped its receiver. Exhausting this cap means the
    // writer survived without ever reaching the kill op (e.g. it went
    // fatal on something queued first) — a real failure to report loudly,
    // not a hang.
    let max_attempts = 4 * rune_db::QUEUE_DEPTH;
    let mut writer_confirmed_gone = false;
    for attempt in 0..=max_attempts {
        match db.store.probe_blocking_for_test(load.doc_id) {
            Ok(_) => assert!(
                attempt < max_attempts,
                "writer never confirmed dead after {max_attempts} blocking probes — \
                 it should have dequeued the kill op long before this"
            ),
            Err(rune_db::Error::WriterGone) => {
                writer_confirmed_gone = true;
                break;
            }
            Err(e) => panic!("unexpected error while awaiting writer death: {e}"),
        }
    }
    assert!(writer_confirmed_gone, "writer death was never confirmed");

    // Exactly ONE super+s now: `trigger_save`'s `materialize_now` enqueues
    // against a writer already confirmed gone, so this single call's
    // enqueue failure — and therefore `on_store_failure` — is deterministic,
    // never racing the in-flight latch `save_in_flight` would otherwise
    // impose on a retry loop.
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(save_key()), &mut effects);

    assert!(
        app.db_banner
            .as_deref()
            .is_some_and(|b| b.contains("recovery disabled")),
        "banner text must read 'recovery disabled: <err>' (got {:?})",
        app.db_banner
    );
    assert!(
        app.db.as_ref().is_some_and(|d| d.degraded),
        "the store must be marked degraded via on_store_failure, not left untouched"
    );
    assert!(
        app.doc(id).unwrap().save_in_flight,
        "WP7: a dead writer must not also make the save itself impossible — the \
         direct-vfs fallback Cmd is in flight, not silently skipped"
    );

    let cmd = effects
        .cmds
        .into_iter()
        .find(|c| c.kind() == rune_tui::runtime::CmdKind::Save)
        .expect("the dead-writer fallback must spawn a direct-vfs Save Cmd");
    let msg = cmd.run().expect("the fallback Cmd must reply");
    let mut effects2 = Effects::default();
    app::update(&mut app, msg, &mut effects2);

    assert!(
        !app.doc(id).unwrap().save_in_flight,
        "the fallback save's own ack must clear save_in_flight"
    );
    assert_eq!(
        vfs.read(doc_path).expect("file still readable"),
        b"hi!",
        "the user's edit must have reached disk despite the dead writer thread"
    );
}

/// Plan WP5.S2/S6's confirm-gate state machine: `super+s` on a degraded
/// store only ARMS the gate (no `materialize` enqueued, `save_in_flight`
/// stays false) the first time; a SECOND `super+s` consumes the gate and
/// actually enqueues the save — mirrors `app::tests::first_quit_press_
/// arms_and_spawns_a_timer_cmd_without_quitting`/`same_chord_twice_quits`'s
/// shape for `pending_quit`. Uses `Store::open_in_memory` (no real file
/// needed) with `Db::degraded` forced `true` by hand — simulating a
/// LATER store failure (plan decision 3), independent of the open ladder's
/// own state, which is exactly what this gate must react to either way.
#[test]
fn super_s_on_a_degraded_store_arms_a_confirm_gate_then_saves_on_second_press() {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let clock: ClockFn = Arc::new(std::time::SystemTime::now);
    let store = Store::open_in_memory(clock, Arc::clone(&vfs), Box::new(|_evt| {}))
        .expect("open in-memory store");
    let bridge = rune_tui::db::DbBridge::bootstrap();
    let db = Db::new(store, bridge, true);
    let doc_db = DocDb::new(1, 0, true, 0);

    let mut app = App::new(
        Buffer::new("hi"),
        Some(PathBuf::from("/doc.md")),
        vfs,
        Some(db),
    );
    let id = app.active;
    app.doc_mut(id).unwrap().db = Some(doc_db);
    dirty_common::force_dirty(&mut app, id); // nothing to save otherwise
    let len = app.doc(id).unwrap().buffer.len();
    app.doc_mut(id).unwrap().cursors = CursorSet::new(len);

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(save_key()), &mut effects);
    assert!(
        app.pending_save_confirm.is_some(),
        "the first super+s on a degraded store must only arm the confirm gate"
    );
    assert!(
        !app.doc(id).unwrap().save_in_flight,
        "no materialize must be enqueued before the gate is confirmed"
    );
    assert!(rune_tui::messages::newest_text(&app).is_some_and(|s| s.contains("recovery disabled")));

    let mut effects2 = Effects::default();
    app::update(&mut app, Msg::Key(save_key()), &mut effects2);
    assert!(
        app.pending_save_confirm.is_none(),
        "the second super+s must consume the confirm gate"
    );
    assert!(
        app.doc(id).unwrap().save_in_flight,
        "the second super+s must actually enqueue the materialize"
    );
}
