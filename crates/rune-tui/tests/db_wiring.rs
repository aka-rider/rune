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
use std::sync::mpsc;

use rune_core::buffer::{AppliedEdit, Buffer};
use rune_core::cursor::CursorSet;
use rune_core::undo::Step;
use rune_db::{ClockFn, DbEvent, LoadResult, OpOutcome, Store, SyncKind, SyncState, Version};
use rune_tui::app::{self, App, StatusSource};
use rune_tui::commands::edit;
use rune_tui::db::{Db, DbBridge, DocDb, PendingOp};
use rune_tui::footer;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{Effects, Msg};
use rune_tui::workspace;
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
    let bridge = DbBridge::bootstrap();
    let (store, _warning) =
        Store::open(db_path, Arc::clone(&vfs), bridge.on_event()).expect("open store");
    let op_id = store.load(doc_path).expect("enqueue load");
    let load_result = match bridge.wait_for_bootstrap_event(|evt| match evt {
        DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == op_id,
        DbEvent::Fatal { .. } => true,
    }) {
        DbEvent::Ok {
            result: OpOutcome::Load(r),
            ..
        } => *r,
        DbEvent::Ok { result, .. } => panic!("unexpected reply to Load: {result:?}"),
        DbEvent::Err { error, .. } => panic!("load failed: {error}"),
        DbEvent::Fatal { error } => panic!("writer thread fatal during load: {error}"),
    };
    (store, bridge, load_result)
}

fn db_from(store: Store, bridge: Arc<DbBridge>, degraded: bool) -> Db {
    Db::new(store, bridge, degraded)
}

fn doc_db_from(load: &LoadResult) -> DocDb {
    DocDb::new(
        load.doc_id,
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
    let db_a = db_from(store_a, bridge_a, false);
    let doc_db_a = doc_db_from(&load_a);

    let mut app_a = App::new(
        Buffer::new(load_a.recovered.clone()),
        Some(doc_path.to_path_buf()),
        Arc::clone(&vfs),
        Some(db_a),
    );
    let id_a = app_a.active;
    app_a.doc_mut(id_a).unwrap().db = Some(doc_db_a);
    let len_a = app_a.doc(id_a).unwrap().buffer.len();
    app_a.doc_mut(id_a).unwrap().cursors = CursorSet::new(len_a);
    for ch in " world".chars() {
        press(&mut app_a, ch);
    }
    assert_eq!(app_a.doc(id_a).unwrap().buffer.content(), "hello world");
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
    let bridge_b = DbBridge::bootstrap();
    let (store_b, _warning) =
        Store::open(&db_path, Arc::clone(&vfs), bridge_b.on_event()).expect("open store b");
    store_b.set_liveness_check(Arc::new(|_pid, _started_at| false));
    let op_id = store_b.load(doc_path).expect("enqueue load b");
    let load_b = match bridge_b.wait_for_bootstrap_event(|evt| match evt {
        DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == op_id,
        DbEvent::Fatal { .. } => true,
    }) {
        DbEvent::Ok {
            result: OpOutcome::Load(r),
            ..
        } => *r,
        DbEvent::Ok { result, .. } => panic!("unexpected reply to Load: {result:?}"),
        DbEvent::Err { error, .. } => panic!("load b failed: {error}"),
        DbEvent::Fatal { error } => panic!("writer b fatal during load: {error}"),
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
    let db_b = db_from(store_b, bridge_b, false);
    let doc_db_b = doc_db_from(&load_b);

    let mut app_b = App::new(
        Buffer::new(load_b.recovered.clone()),
        Some(doc_path.to_path_buf()),
        Arc::clone(&vfs),
        Some(db_b),
    );
    let id_b = app_b.active;
    app_b.doc_mut(id_b).unwrap().db = Some(doc_db_b);
    if let Some(bridge_edit) = bridge_edit {
        app_b.doc_mut(id_b).unwrap().journal.push(Step {
            edits: vec![bridge_edit],
            cursors_before: Vec::new(),
            cursors_after: Vec::new(),
        });
    }

    assert_eq!(app_b.doc(id_b).unwrap().buffer.content(), "hello world");

    edit::undo(&mut app_b, id_b);
    assert_eq!(
        app_b.doc(id_b).unwrap().buffer.content(),
        "hello",
        "post-restart undo must reach the pre-crash anchor (the disk content)"
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
/// for the writer to be CONFIRMED gone (a side-channel `probe` enqueue
/// returning `Err`) before pressing save exactly once, rather than racing
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
/// needed) with `Db::degraded` forced `true` by hand — simulating a
/// LATER store failure (plan decision 3), independent of the open ladder's
/// own state, which is exactly what this gate must react to either way.
#[test]
fn super_s_on_a_degraded_store_arms_a_confirm_gate_then_saves_on_second_press() {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let clock: ClockFn = Arc::new(std::time::SystemTime::now);
    let store = Store::open_in_memory(clock, Arc::clone(&vfs), Box::new(|_evt| {}))
        .expect("open in-memory store");
    let bridge = DbBridge::bootstrap();
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
    app.doc_mut(id).unwrap().saved_version = 0; // force dirty — nothing to save otherwise
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
        app.doc(id).unwrap().save_in_flight,
        "the second super+s must actually enqueue the materialize"
    );
}

/// Opens a real `Store` at a fresh temp dir and wires it onto a brand-new
/// `App` with exactly one untitled draft (no path) — the state
/// `workspace::open_path`'s WP6 hydration runs against, since it only ever
/// hydrates the document it opens, never the app's initial one. The
/// returned bridge is left in its `Bootstrap` sink (never `attach`ed), so
/// every `DbEvent` — including the `Load` ack `open_path` enqueues — stays
/// buffered on the bridge itself for the test to drain through
/// `recv_ok`/`app::update`, exactly like `open_and_load` above does for
/// bootstrap hydration.
fn app_with_store(label: &str, vfs: Arc<dyn Vfs + Send + Sync>) -> (App, Arc<DbBridge>) {
    let dir = temp_db_dir(label);
    let db_path = dir.join("rune-v1.db");
    let bridge = DbBridge::bootstrap();
    let (store, _warning) =
        Store::open(&db_path, Arc::clone(&vfs), bridge.on_event()).expect("open store");
    let db = Db::new(store, Arc::clone(&bridge), false);
    let app = App::new(Buffer::new(""), None, vfs, Some(db));
    (app, bridge)
}

/// Blocks for the next `DbEvent::Ok` reply to `op_id` buffered on `bridge`,
/// panicking on an `Err`/`Fatal`/mismatched-id reply — the same shape
/// `open_and_load` above uses for bootstrap's own `Load` ack.
fn recv_ok(bridge: &DbBridge, op_id: u64) -> OpOutcome {
    match bridge.wait_for_bootstrap_event(|evt| match evt {
        DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == op_id,
        DbEvent::Fatal { .. } => true,
    }) {
        DbEvent::Ok { result, .. } => result,
        DbEvent::Err { id, error } => panic!("op {id} failed: {error}"),
        DbEvent::Fatal { error } => panic!("writer thread fatal: {error}"),
    }
}

/// Plan WP6.S6: opening an Explorer path enqueues exactly one `Load` op and
/// records it in `app.db_ops`, keyed to the newly opened document — not the
/// app's pre-existing untitled draft.
#[test]
fn open_path_enqueues_exactly_one_load_op_and_records_it_in_db_ops() {
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, _rx) = app_with_store("open-path-enqueue", vfs);
    let initial_id = app.active;

    workspace::open_path(&mut app, Path::new("/doc.md"));

    let opened_id = app.active;
    assert_ne!(
        opened_id, initial_id,
        "open_path must switch to the newly opened document"
    );
    assert_eq!(
        app.db_ops.len(),
        1,
        "open_path must enqueue exactly one op (the Load)"
    );
    assert_eq!(
        app.db_ops.values().next().map(|pending| pending.doc),
        Some(opened_id),
        "the enqueued op must be routed to the opened document, not the initial draft"
    );
    assert!(
        app.doc(opened_id).unwrap().db.is_none(),
        "db stays None until the Load ack lands"
    );
}

/// The `Load` ack installs `Document::db` as `Some` once it lands.
#[test]
fn load_ack_installs_document_db_as_some() {
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, rx) = app_with_store("open-path-ack-installs-db", vfs);
    workspace::open_path(&mut app, Path::new("/doc.md"));
    let id = app.active;
    let op_id = *app.db_ops.keys().next().expect("one op enqueued");

    let result = recv_ok(&rx, op_id);
    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Db(DbEvent::Ok { id: op_id, result }),
        &mut effects,
    );

    assert!(
        app.doc(id).unwrap().db.is_some(),
        "a Load ack with a saved_obs baseline must install DocDb"
    );
    assert!(
        !app.db_ops.contains_key(&op_id),
        "the ack must pop its own db_ops entry"
    );
    assert_eq!(
        app.doc(id).unwrap().buffer.content(),
        "hello",
        "no divergence to recover: the buffer stays exactly what was read off disk"
    );
}

/// Data-safety guard (plan WP6.S3): an ack for a document the user kept
/// typing into during the async round trip must NEVER clobber those
/// keystrokes — the buffer bytes stay exactly as typed, even though the
/// ack's own `recovered` content would otherwise differ from what's now on
/// screen. `DocDb` is still installed: the document's own recovery journal
/// is real and should be used going forward.
#[test]
fn ack_for_a_document_edited_during_the_round_trip_leaves_the_buffer_unchanged() {
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, rx) = app_with_store("open-path-edited-in-flight", vfs);
    workspace::open_path(&mut app, Path::new("/doc.md"));
    let id = app.active;
    let op_id = *app.db_ops.keys().next().expect("one op enqueued");

    // The user types while the Load round trip is still in flight — this
    // bumps the buffer's version past what was recorded at enqueue time.
    let len = app.doc(id).unwrap().buffer.len();
    app.doc_mut(id).unwrap().cursors = CursorSet::new(len);
    press(&mut app, '!');
    assert_eq!(app.doc(id).unwrap().buffer.content(), "hello!");

    let result = recv_ok(&rx, op_id);
    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Db(DbEvent::Ok { id: op_id, result }),
        &mut effects,
    );

    assert_eq!(
        app.doc(id).unwrap().buffer.content(),
        "hello!",
        "the ack must never clobber a keystroke typed during the round trip"
    );
    assert!(
        app.doc(id).unwrap().db.is_some(),
        "DocDb must still be installed even when the buffer adopt is skipped"
    );
}

/// A `Load` ack whose `LoadResult` carries no `saved_obs` baseline (should
/// not occur in practice — see `LoadResult::saved_obs`'s own doc comment;
/// exercised here directly since a real `Store::load` always adopts one on
/// a first load) must install nothing and surface a status message rather
/// than binding a document to a recovery row with no CAS baseline.
#[test]
fn ack_with_no_saved_obs_leaves_db_none_and_sets_a_status_message() {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let mut app = App::new(
        Buffer::new("hello"),
        Some(PathBuf::from("/doc.md")),
        vfs,
        None,
    );
    let id = app.active;

    let op_id = 1u64;
    let issued_version = app.doc(id).unwrap().buffer.version();
    app.db_ops.insert(op_id, PendingOp::load(id, issued_version));

    let load_result = LoadResult {
        doc_id: 1,
        renamed_from: None,
        disk_content: "hello".to_string(),
        recovered: "hello".to_string(),
        has_history: false,
        sync: SyncState {
            kind: SyncKind::Clean,
            ancestor: None,
            ours: Version {
                hash: String::new(),
                obs: None,
            },
            theirs: None,
        },
        nlink: 1,
        saved_obs: None,
        bridge_seq: None,
    };

    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Db(DbEvent::Ok {
            id: op_id,
            result: OpOutcome::Load(Box::new(load_result)),
        }),
        &mut effects,
    );

    assert!(
        app.doc(id).unwrap().db.is_none(),
        "no baseline observation means no DocDb binding"
    );
    assert_eq!(app.doc(id).unwrap().buffer.content(), "hello");
    assert_eq!(app.status_source, StatusSource::Other);
    assert!(
        app.status_message
            .as_deref()
            .is_some_and(|s| s.contains("no baseline observation")),
        "a status message must explain why crash recovery wasn't bound (got {:?})",
        app.status_message
    );
}

/// Review fix (plan WP5.S2, [rune-tui A 3]): `handle_load_ack` must refuse
/// to adopt recovered content that would empty (or drastically shrink) a
/// non-empty on-disk file — the §1.3 destructive-async-reset suspicion
/// check, run through the shared `Document::hydrate` chokepoint. The buffer
/// stays exactly what was on disk, and a status message explains why.
#[test]
fn ack_refuses_to_adopt_recovered_content_that_would_empty_the_disk_content() {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let disk_content = "a whole paragraph of real content that must not vanish";
    let mut app = App::new(
        Buffer::new(disk_content),
        Some(PathBuf::from("/doc.md")),
        vfs,
        None,
    );
    let id = app.active;

    let op_id = 1u64;
    let issued_version = app.doc(id).unwrap().buffer.version();
    app.db_ops.insert(op_id, PendingOp::load(id, issued_version));

    let load_result = LoadResult {
        doc_id: 1,
        renamed_from: None,
        disk_content: disk_content.to_string(),
        // A suspicious "recovered" empty string — the exact destructive
        // async-reset pattern §1.3 forbids adopting silently.
        recovered: String::new(),
        has_history: false,
        sync: SyncState {
            kind: SyncKind::Clean,
            ancestor: None,
            ours: Version {
                hash: String::new(),
                obs: None,
            },
            theirs: None,
        },
        nlink: 1,
        saved_obs: Some(1),
        bridge_seq: None,
    };

    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Db(DbEvent::Ok {
            id: op_id,
            result: OpOutcome::Load(Box::new(load_result)),
        }),
        &mut effects,
    );

    assert_eq!(
        app.doc(id).unwrap().buffer.content(),
        disk_content,
        "a refused hydration must leave the buffer exactly as it was on disk"
    );
    assert!(
        !app.doc(id).unwrap().is_dirty(),
        "a refused hydration must not mark the buffer dirty"
    );
    assert!(
        app.doc(id).unwrap().db.is_some(),
        "DocDb must still be installed even when the adopt is refused"
    );
    assert_eq!(app.status_source, StatusSource::Other);
    assert!(
        app.status_message
            .as_deref()
            .is_some_and(|s| s.contains("crash recovery")),
        "a status message must explain the refusal (got {:?})",
        app.status_message
    );
}

/// Plan WP3.S1/S4's regression test: a two-file CLI launch opens BOTH extra
/// documents (`workspace::open_path`, exactly as `rune-cli::main`'s
/// extra-positional loop does) before `DbBridge::attach` ever runs — the
/// same bridge is still in its `Bootstrap` sink for the whole window. Before
/// the fix, any `Load` ack landing in that window went to an `mpsc::Sender`
/// whose paired receiver bootstrap hydration had already dropped, and was
/// silently lost (`let _ = tx.send(evt)`): the tab kept `db: None` all
/// session. Both documents here must still end up with `db: Some(..)` once
/// `attach` finally runs and drains what accumulated.
///
/// Deterministic, no wall-clock sleep: a THIRD op (`probe` against an
/// unrelated, already-hydrated document) is enqueued strictly AFTER both
/// documents' `Load`s. The writer thread is a single ordered FIFO (`db.rs`'s
/// own module doc) that posts each op's event before starting the next, so
/// blocking on the probe's own ack (`wait_for_bootstrap_event`) is a
/// genuine rendezvous proving both earlier `Load` acks are already sitting
/// in the bridge's `Bootstrap` buffer — never a race, never a poll loop.
#[test]
fn two_file_launch_delivers_both_load_acks_once_attach_drains_the_bootstrap_buffer() {
    let mem = Mem::new();
    publish(&mem, Path::new("/marker.md"), b"marker");
    publish(&mem, Path::new("/a.md"), b"a content");
    publish(&mem, Path::new("/b.md"), b"b content");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let dir = temp_db_dir("two-file-handover");
    let db_path = dir.join("rune-v1.db");
    let bridge = DbBridge::bootstrap();
    let (store, _warning) =
        Store::open(&db_path, Arc::clone(&vfs), bridge.on_event()).expect("open store");

    // Synchronously hydrate an unrelated marker document — purely to mint a
    // valid `doc_id` the FIFO-order probe below can target; not part of the
    // two-file scenario under test.
    let marker_op = store
        .load(Path::new("/marker.md"))
        .expect("enqueue marker load");
    let marker_doc_id = match bridge.wait_for_bootstrap_event(|evt| match evt {
        DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == marker_op,
        DbEvent::Fatal { .. } => true,
    }) {
        DbEvent::Ok {
            result: OpOutcome::Load(load),
            ..
        } => load.doc_id,
        other => panic!("expected a Load ack for the marker doc, got {other:?}"),
    };

    // `App::new_untitled` mirrors the CLI's own no-positional-file shape —
    // the bridge is still `Bootstrap` here, matching `rune-cli::main`'s
    // window between `Store::open` and `runtime::run`'s `attach` call.
    let mut app = App::new_untitled(Arc::clone(&vfs));
    app.db = Some(Db::new(store, Arc::clone(&bridge), false));

    // Exactly `rune-cli::main`'s extra-positional loop: every file after
    // the first opens through `workspace::open_path`, enqueueing its own
    // `Load` — both land in the still-`Bootstrap` bridge.
    let id_a = workspace::open_path(&mut app, Path::new("/a.md")).expect("open a");
    let id_b = workspace::open_path(&mut app, Path::new("/b.md")).expect("open b");
    assert!(app.doc(id_a).unwrap().db.is_none(), "no ack has landed yet");
    assert!(app.doc(id_b).unwrap().db.is_none(), "no ack has landed yet");

    // Enqueued strictly after both Loads — see the FIFO-ordering doc
    // comment above.
    let probe_op = app
        .db
        .as_ref()
        .expect("app has a store")
        .store
        .probe(marker_doc_id)
        .expect("enqueue probe");
    let _ = bridge.wait_for_bootstrap_event(|evt| match evt {
        DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == probe_op,
        DbEvent::Fatal { .. } => true,
    });

    // The handover itself: `runtime::run`'s one call site.
    let (tx, rx) = mpsc::channel::<Msg>();
    bridge.attach(tx);

    let mut effects = Effects::default();
    for msg in rx.try_iter() {
        app::update(&mut app, msg, &mut effects);
    }

    assert!(
        app.doc(id_a).unwrap().db.is_some(),
        "doc a's Load ack, buffered before attach, must still be delivered"
    );
    assert!(
        app.doc(id_b).unwrap().db.is_some(),
        "doc b's Load ack, buffered before attach, must still be delivered"
    );
}
