//! Shared setup helpers for the rune-tui <-> rune-db wiring test suite,
//! split across `db_wiring_degraded.rs` (the degraded-store banner and its
//! confirm gate), `db_wiring_hydrate.rs` (restart hydration and Load-ack
//! adoption), and `db_wiring_lifecycle.rs` (open/close op bookkeeping and
//! the bootstrap handover) — TODO.md's 500-line budget split of the original
//! `db_wiring.rs`. Each consumer pulls this in via `mod db_wiring_common;`
//! — integration test files are separate binaries, so this is the one
//! place all three draw an identical `Store`/`App` fixture from, rather
//! than risking drift.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_db::{DbEvent, LoadResult, OpOutcome, Store};
use rune_tui::app::{self, App};
use rune_tui::db::{Db, DbBridge, DocDb};
use rune_tui::document::DocumentId;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::{Mem, Vfs};

/// A per-process monotonic counter, folded into [`temp_db_dir`]'s own name
/// alongside its wall-clock nanosecond reading — the clock alone is not a
/// reliable uniqueness source: two test THREADS in the same binary racing
/// through this call can land on the same clock tick if the platform's
/// actual resolution is coarser than a true nanosecond, and a real filename
/// collision means two logically unrelated tests silently share one SQLite
/// file (and, through it, `documents`/`observations` rows for the same
/// "/doc.md" path) — exactly what this counter exists to rule out.
static TEMP_DB_DIR_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn temp_db_dir(label: &str) -> PathBuf {
    let seq = TEMP_DB_DIR_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "rune-tui-db-wiring-{label}-{}-{}-{seq}",
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
pub fn publish(vfs: &Mem, path: &Path, bytes: &[u8]) {
    let temp = vfs.write_durable(path, bytes).expect("write_durable");
    vfs.rename_excl(&temp, path).expect("publish");
}

/// Opens a `Store` at `db_path` and hydrates `doc_path` through it,
/// blocking for the `Load` ack on the bridge's own bootstrap channel —
/// mirrors `rune-cli::main::bootstrap_db`.
pub fn open_and_load(
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

pub fn db_from(store: Store, bridge: Arc<DbBridge>, degraded: bool) -> Db {
    Db::new(store, bridge, degraded)
}

pub fn doc_db_from(load: &LoadResult) -> DocDb {
    DocDb::new(
        load.doc_id,
        false, // bind_new: the doc already exists on disk in every test here
        0,
    )
}

/// The [`doc_db_from`] counterpart for the shared per-file CAS baseline:
/// joins `App::file_bindings` for `load.doc_id`, seeded from
/// `load.saved_obs`, exactly like `db_ack::handle_load_ack`'s own production
/// call to `App::install_or_join_file_binding` does. Every test that installs a `doc_db_from`
/// binding onto a live `App` must call this too, or that document's CAS
/// baseline reads back as the unseeded default instead of what its own
/// fixture `Load` actually observed.
pub fn join_binding_from(app: &mut App, load: &LoadResult) {
    app.install_or_join_file_binding(load.doc_id, load.saved_obs.unwrap_or(0));
}

pub fn press(app: &mut App, ch: char) {
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

pub fn save_key() -> KeyInput {
    KeyInput {
        code: KeyCode::Char('s'),
        mods: Mods {
            sup: true,
            ..Mods::NONE
        },
    }
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
pub fn app_with_store(label: &str, vfs: Arc<dyn Vfs + Send + Sync>) -> (App, Arc<DbBridge>) {
    let dir = temp_db_dir(label);
    let db_path = dir.join("rune-v1.db");
    let bridge = DbBridge::bootstrap();
    let (store, _warning) =
        Store::open(&db_path, Arc::clone(&vfs), bridge.on_event()).expect("open store");
    let db = Db::new(store, Arc::clone(&bridge), false);
    let app = App::new(Buffer::new(""), None, vfs, Some(db));
    (app, bridge)
}

/// Drains the single op currently recorded in `app.db_ops` for `doc`,
/// feeding its ack through `app::update` exactly as the real runtime loop
/// would when the op's `DbEvent` arrives on `Msg::Db` — `Err` events
/// included, so a caller asserting on a failure path gets the event back
/// rather than a panic.
pub fn drain_one_op_for(app: &mut App, bridge: &DbBridge, doc: DocumentId) -> DbEvent {
    let op_id = *app
        .db_ops
        .iter()
        .find(|(_, pending)| pending.doc == doc)
        .expect("one op recorded for this document")
        .0;
    let evt = bridge.wait_for_bootstrap_event(|evt| match evt {
        DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == op_id,
        DbEvent::Fatal { .. } => true,
    });
    let mut effects = Effects::default();
    app::update(app, Msg::Db(evt.clone()), &mut effects);
    evt
}

/// Blocks for the next `DbEvent::Ok` reply to `op_id` buffered on `bridge`,
/// panicking on an `Err`/`Fatal`/mismatched-id reply — the same shape
/// `open_and_load` above uses for bootstrap's own `Load` ack.
pub fn recv_ok(bridge: &DbBridge, op_id: u64) -> OpOutcome {
    match bridge.wait_for_bootstrap_event(|evt| match evt {
        DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == op_id,
        DbEvent::Fatal { .. } => true,
    }) {
        DbEvent::Ok { result, .. } => result,
        DbEvent::Err { id, error } => panic!("op {id} failed: {error}"),
        DbEvent::Fatal { error } => panic!("writer thread fatal: {error}"),
    }
}
