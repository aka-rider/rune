//! WP5 "Done when" integration tests for the rune-tui <-> rune-db wiring:
//! async replica journaling, the degraded-store banner (plan decision 3),
//! and post-restart hydration/undo (plan WP5.S4) — replacing the plan's
//! interactive manual gate. Mirrors `rune-cli::main`'s own bootstrap
//! sequence (`Store::open` -> `load` -> block for its ack on the bootstrap
//! channel) since that logic is private to the `rune-cli` binary crate.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_core::buffer::{AppliedEdit, Buffer};
use rune_core::cursor::CursorSet;
use rune_core::undo::Step;
use rune_db::{ClockFn, DbEvent, LoadResult, OpOutcome, Store};
use rune_tui::app::{self, App};
use rune_tui::commands::edit;
use rune_tui::db::{AppDb, DbBridge};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{Effects, Msg};
use rune_tui::status;
use rune_vfs::{Mem, Vfs};

fn temp_db_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rune-tui-db-wiring-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// The same atomic-publish shape `rune-db`'s own tests use: `write_durable`
/// + `rename_excl`.
fn publish(vfs: &Mem, path: &Path, bytes: &[u8]) {
    let temp = vfs.write_durable(path, bytes).expect("write_durable");
    vfs.rename_excl(&temp, path).expect("publish");
}

/// Opens a `Store` at `db_path` and hydrates `doc_path` through it,
/// blocking for the `Load` ack on the bridge's own bootstrap channel —
/// mirrors `rune-cli::main::bootstrap_db`.
fn open_and_load(
    db_path: &Path,
    vfs: Arc<dyn Vfs + Send + Sync>,
    doc_path: &Path,
) -> (Store, Arc<DbBridge>, LoadResult) {
    let (bridge, rx) = DbBridge::bootstrap();
    let (store, _warning) =
        Store::open(db_path, Arc::clone(&vfs), bridge.on_event()).expect("open store");
    let op_id = store.load(doc_path).expect("enqueue load");
    let load_result = loop {
        match rx.recv().expect("writer thread alive during hydration") {
            DbEvent::Ok { id, result } if id == op_id => match result {
                OpOutcome::Load(r) => break *r,
                other => panic!("unexpected reply to Load: {other:?}"),
            },
            DbEvent::Err { id, error } if id == op_id => panic!("load failed: {error}"),
            DbEvent::Fatal { error } => panic!("writer thread fatal during load: {error}"),
            _ => continue,
        }
    };
    (store, bridge, load_result)
}

fn app_db_from(store: Store, bridge: Arc<DbBridge>, load: &LoadResult, degraded: bool) -> AppDb {
    AppDb::new(
        store,
        bridge,
        load.doc_id,
        degraded,
        load.saved_obs.unwrap_or(0),
        false, // bind_new: the doc already exists on disk in every test here
        0,
    )
}

fn press(app: &mut App, ch: char) {
    let mut effects = Effects::default();
    app::update(
        app,
        Msg::Key(KeyInput {
            code: KeyCode::Char(ch),
            mods: Mods::NONE,
        }),
        &mut effects,
    );
}

/// Plan WP5 "Done when": type -> kill the store writer via the test hook
/// (`Store::kill_writer_for_test`) -> the persistent degraded banner
/// appears in `status::status_text`'s output, and the buffer's content is
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
    let app_db = app_db_from(store, bridge, &load, false);

    let mut app = App::new(
        Buffer::new(load.recovered.clone()),
        Some(doc_path.to_path_buf()),
        vfs,
        Some(app_db),
    );
    app.editor.cursors = CursorSet::new(app.editor.buffer.len());

    // Typing while the store is healthy journals normally — no banner.
    press(&mut app, '!');
    assert_eq!(app.editor.buffer.content(), "hi!");
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
        status::status_text(&app).contains("recovery disabled"),
        "the banner must be part of the rendered status line"
    );
    assert!(
        app.db.as_ref().is_some_and(|d| d.degraded),
        "the store must be marked degraded"
    );
    // No rollback, ever (plan decision 3): the buffer must reflect EVERY
    // keystroke typed so far, regardless of exactly when the writer died
    // relative to these presses.
    assert_eq!(
        app.editor.buffer.content(),
        typed,
        "a store failure must never roll back the in-memory buffer"
    );
}

/// Plan WP5 "Done when" (replaces the interactive manual gate): edits
/// journaled by one session -> a NEW `Store` opened on the SAME db path
/// (a simulated restart) hydrates the recovered content, and undo reaches
/// the pre-crash anchor.
#[test]
fn restart_hydrates_content_and_undo_reaches_the_anchor() {
    let dir = temp_db_dir("restart");
    let db_path = dir.join("rune-v1.db");
    let doc_path = Path::new("/doc.md");

    let mem = Mem::new();
    publish(&mem, doc_path, b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    // Session A: types more, never saves (materializes) to disk.
    let (store_a, bridge_a, load_a) = open_and_load(&db_path, Arc::clone(&vfs), doc_path);
    assert_eq!(load_a.disk_content, "hello");
    assert_eq!(load_a.recovered, "hello");
    let app_db_a = app_db_from(store_a, bridge_a, &load_a, false);

    let mut app_a = App::new(
        Buffer::new(load_a.recovered.clone()),
        Some(doc_path.to_path_buf()),
        Arc::clone(&vfs),
        Some(app_db_a),
    );
    app_a.editor.cursors = CursorSet::new(app_a.editor.buffer.len());
    for ch in " world".chars() {
        press(&mut app_a, ch);
    }
    assert_eq!(app_a.editor.buffer.content(), "hello world");
    assert!(
        app_a.db_banner.is_none(),
        "session A's own store must stay healthy throughout"
    );

    // Every journaled edit must be durably committed before "restarting" —
    // `Store::shutdown` drains its writer FIFO to empty before returning
    // (deterministic; no polling needed).
    let store_a = app_a.db.take().expect("app_a has a store").store;
    store_a.shutdown();

    // Session B (simulated restart): a brand-new `Store` on the SAME path.
    // Both "sessions" here share one OS process/pid, so the real liveness
    // check (which would see this very test process as alive) can't tell
    // them apart — override it to report session A dead, the documented,
    // supported way to simulate a genuinely dead session
    // (`Store::set_liveness_check`).
    let (bridge_b, rx_b) = DbBridge::bootstrap();
    let (store_b, _warning) =
        Store::open(&db_path, Arc::clone(&vfs), bridge_b.on_event()).expect("open store b");
    store_b.set_liveness_check(Arc::new(|_pid, _started_at| false));
    let op_id = store_b.load(doc_path).expect("enqueue load b");
    let load_b = loop {
        match rx_b.recv().expect("writer b alive during hydration") {
            DbEvent::Ok { id, result } if id == op_id => match result {
                OpOutcome::Load(r) => break *r,
                other => panic!("unexpected reply to Load: {other:?}"),
            },
            DbEvent::Err { id, error } if id == op_id => panic!("load b failed: {error}"),
            DbEvent::Fatal { error } => panic!("writer b fatal during load: {error}"),
            _ => continue,
        }
    };

    assert_eq!(
        load_b.recovered, "hello world",
        "restart must recover session A's unsaved edits"
    );
    assert_eq!(
        load_b.disk_content, "hello",
        "the on-disk file itself was never touched — session A never saved"
    );

    // The same synthetic bridge-edit reconstruction `rune-cli::main` does
    // (plan WP5.S4) — seeds the LOCAL undo journal so undo reaches the
    // anchor in one step.
    let bridge_edit = (load_b.recovered != load_b.disk_content).then(|| AppliedEdit {
        start: 0,
        end: load_b.disk_content.len(),
        deleted: load_b.disk_content.clone(),
        insert: load_b.recovered.clone(),
    });
    let app_db_b = app_db_from(store_b, bridge_b, &load_b, false);

    let mut app_b = App::new(
        Buffer::new(load_b.recovered.clone()),
        Some(doc_path.to_path_buf()),
        Arc::clone(&vfs),
        Some(app_db_b),
    );
    if let Some(bridge_edit) = bridge_edit {
        app_b.editor.journal.push(Step {
            edits: vec![bridge_edit],
            cursors_before: Vec::new(),
            cursors_after: Vec::new(),
        });
    }

    assert_eq!(app_b.editor.buffer.content(), "hello world");

    edit::undo(&mut app_b);
    assert_eq!(
        app_b.editor.buffer.content(),
        "hello",
        "post-restart undo must reach the pre-crash anchor (the disk content)"
    );
}

/// Finding 5: a `materialize` enqueue failure (the store writer confirmed
/// gone) must degrade the store and raise the sticky banner through the
/// SAME `on_store_failure` chokepoint `append_edit`/`move_undo_pos` use —
/// not a one-shot `SaveError` status that leaves `db.degraded` untouched
/// and lets the next `super+s` silently retry against an already-dead
/// writer. Deterministically waits for the writer to be CONFIRMED gone (a
/// side-channel `probe` enqueue returning `Err`) before pressing save
/// exactly once, rather than racing `super+s`'s own in-flight latch against
/// the kill op's async dequeue.
#[test]
fn killed_writer_makes_materialize_enqueue_degrade_the_store_synchronously() {
    let dir = temp_db_dir("kill-writer-materialize");
    let db_path = dir.join("rune-v1.db");
    let doc_path = Path::new("/doc.md");

    let mem = Mem::new();
    publish(&mem, doc_path, b"hi");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (store, bridge, load) = open_and_load(&db_path, Arc::clone(&vfs), doc_path);
    let app_db = app_db_from(store, bridge, &load, false);

    let mut app = App::new(
        Buffer::new(load.recovered.clone()),
        Some(doc_path.to_path_buf()),
        vfs,
        Some(app_db),
    );
    app.editor.cursors = CursorSet::new(app.editor.buffer.len());

    // Dirty the buffer (a healthy edit — the writer is still alive here) so
    // `trigger_save` below actually has something to save.
    press(&mut app, '!');
    assert!(app.db_banner.is_none());

    let db = app.db.as_ref().expect("app has a store");
    db.store
        .kill_writer_for_test()
        .expect("enqueue the kill op");

    // Bounded spin, not a wall-clock sleep (repo convention): the kill op
    // must first be DEQUEUED by the writer thread before `try_send` starts
    // observing `WriterGone` — poll with a cheap, repeatable, non-latching
    // op (`probe`, which carries no in-flight guard) until the writer is
    // CONFIRMED gone.
    let mut confirmed_gone = false;
    for _ in 0..20_000 {
        if db.store.probe(load.doc_id).is_err() {
            confirmed_gone = true;
            break;
        }
    }
    assert!(
        confirmed_gone,
        "the writer must eventually be confirmed gone"
    );

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
        !app.save_in_flight,
        "on_store_failure must clear save_in_flight on an enqueue failure"
    );
}

fn save_key() -> KeyInput {
    KeyInput {
        code: KeyCode::Char('s'),
        mods: Mods {
            sup: true,
            ..Mods::NONE
        },
    }
}

/// Plan WP5.S2/S6's confirm-gate state machine: `super+s` on a degraded
/// store only ARMS the gate (no `materialize` enqueued, `save_in_flight`
/// stays false) the first time; a SECOND `super+s` consumes the gate and
/// actually enqueues the save — mirrors `app::tests::first_quit_press_
/// arms_and_spawns_a_timer_cmd_without_quitting`/`same_chord_twice_quits`'s
/// shape for `pending_quit`. Uses `Store::open_in_memory` (no real file
/// needed) with `AppDb::degraded` forced `true` by hand — simulating a
/// LATER store failure (plan decision 3), independent of the open ladder's
/// own state, which is exactly what this gate must react to either way.
#[test]
fn super_s_on_a_degraded_store_arms_a_confirm_gate_then_saves_on_second_press() {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let clock: ClockFn = Arc::new(std::time::SystemTime::now);
    let store = Store::open_in_memory(clock, Arc::clone(&vfs), Box::new(|_evt| {}))
        .expect("open in-memory store");
    let (bridge, _rx) = DbBridge::bootstrap();
    let mut app_db = AppDb::new(store, bridge, 1, false, 0, true, 0);
    app_db.degraded = true;

    let mut app = App::new(
        Buffer::new("hi"),
        Some(PathBuf::from("/doc.md")),
        vfs,
        Some(app_db),
    );
    app.saved_version = 0; // force dirty — nothing to save otherwise
    app.editor.cursors = CursorSet::new(app.editor.buffer.len());

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(save_key()), &mut effects);
    assert!(
        app.pending_save_confirm.is_some(),
        "the first super+s on a degraded store must only arm the confirm gate"
    );
    assert!(
        !app.save_in_flight,
        "no materialize must be enqueued before the gate is confirmed"
    );
    assert!(
        app.status_message
            .as_deref()
            .is_some_and(|s| s.contains("recovery disabled"))
    );

    let mut effects2 = Effects::default();
    app::update(&mut app, Msg::Key(save_key()), &mut effects2);
    assert!(
        app.pending_save_confirm.is_none(),
        "the second super+s must consume the confirm gate"
    );
    assert!(
        app.save_in_flight,
        "the second super+s must actually enqueue the materialize"
    );
}
