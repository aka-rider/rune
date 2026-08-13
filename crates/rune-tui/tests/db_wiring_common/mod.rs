//! Shared fixtures for the `db_wiring_*`, `merge_*`, and save-epoch test
//! suites: real-`Store` construction at a temp path, the atomic-publish
//! shape, and the bootstrap-bridge drain primitives. Everything a
//! `rune_fuzz::Session` already provides (App construction, key delivery,
//! op draining as checked steps) lives there, not here.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_db::{DbEvent, OpOutcome, Store};
use rune_tui::app::App;
use rune_tui::db::{Db, DbBridge};
use rune_vfs::Vfs;

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
pub fn publish(vfs: &dyn Vfs, path: &Path, bytes: &[u8]) {
    let temp = vfs.write_durable(path, bytes).expect("write_durable");
    vfs.rename_excl(&temp, path).expect("publish");
}

/// Opens a fresh real `Store` at `db_path` with a bootstrap-mode bridge —
/// the two halves `Session::open_with_db` and a bespoke `Db::new` both
/// start from.
pub fn store_at(db_path: &Path, vfs: Arc<dyn Vfs + Send + Sync>) -> (Store, Arc<DbBridge>) {
    let bridge = DbBridge::bootstrap();
    let (store, _warning) = Store::open(db_path, vfs, bridge.on_event()).expect("open store");
    (store, bridge)
}

/// [`store_at`], with the liveness check overridden to report every other
/// session dead — the documented, supported way to simulate a restart after
/// a crash when both "sessions" share this one OS process/pid
/// (`Store::set_liveness_check`).
pub fn restarted_store_at(
    db_path: &Path,
    vfs: Arc<dyn Vfs + Send + Sync>,
) -> (Store, Arc<DbBridge>) {
    let (store, bridge) = store_at(db_path, vfs);
    store.set_liveness_check(Arc::new(|_pid, _started_at| false));
    (store, bridge)
}

/// Blocks for the next `DbEvent::Ok` reply to `op_id` buffered on `bridge`,
/// panicking on an `Err`/`Fatal`/mismatched-id reply.
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

/// Opens a real `Store` at a fresh temp dir and wires it onto a brand-new
/// `App` with exactly one untitled draft (no path) — the state
/// `workspace::open_path`'s WP6 hydration runs against, since it only ever
/// hydrates the document it opens, never the app's initial one. The
/// returned bridge is left in its `Bootstrap` sink (never `attach`ed), so
/// every `DbEvent` — including the `Load` ack `open_path` enqueues — stays
/// buffered on the bridge itself for the test to drain through `recv_ok`.
pub fn app_with_store(label: &str, vfs: Arc<dyn Vfs + Send + Sync>) -> (App, Arc<DbBridge>) {
    let dir = temp_db_dir(label);
    let (store, bridge) = store_at(&dir.join("rune-v1.db"), Arc::clone(&vfs));
    let db = Db::new(store, Arc::clone(&bridge), false);
    let app = App::new(Buffer::new(""), None, vfs, Some(db));
    (app, bridge)
}
