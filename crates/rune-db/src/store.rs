//! `Store`: the public handle over `rune-db`'s writer/reader threads and
//! this process's own session identity. This is the ONLY type the rest of
//! the workspace (`rune-tui`, WP5) is meant to touch — no table-level CRUD
//! escapes the crate (plan decision 11); domain verbs land here as WP3+
//! grows `OpKind` and the reader's request enum.
//!
//! # Open ladder
//!
//! 1. Open `path` directly (creating it if missing).
//! 2. On failure: `mkdir_all(path.parent())`, retry step 1.
//! 3. On failure: fall back to a private, process-unique in-memory database
//!    and set `degraded = true`.
//!
//! Establishing this process's own `sessions` row is the one remaining hard
//! failure past that point (plan decision — session identity is
//! load-bearing for every subsequent write); there is no fallback left
//! below `:memory:`.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use rusqlite::Connection;

use rune_vfs::Vfs;

use crate::open_ladder::{LadderResult, memory_uri, open_ladder, open_memory_backed};
use crate::writer::{OnEvent, OpKind, WriteOp};
use crate::{Error, reader, retry, session, writer};

/// An injectable wall clock (plan Gotchas: "Wall-clock coalescing is
/// nondeterministic in tests — rune-db must take a `clock: ... -> SystemTime`
/// injection"). Production uses `SystemTime::now`; tests install a
/// deterministic stand-in.
pub type ClockFn = Arc<dyn Fn() -> SystemTime + Send + Sync>;

/// An injectable liveness check: `(pid, proc_started_at) -> still running?`.
/// Production uses [`session::is_process_alive`]; tests simulate a dead
/// session deterministically.
pub type LivenessCheckFn = Arc<dyn Fn(i64, &str) -> bool + Send + Sync>;

/// The default synchronous busy-of-storage warning surfaced when the open
/// ladder bottoms out at the in-memory fallback.
pub const DEGRADED_WARNING: &str = "history disabled — storage unavailable";

pub struct Store {
    writer: writer::WriterHandle,
    reader: reader::ReaderHandle,
    degraded: bool,
    pub(crate) session_id: i64,
    next_op_id: AtomicU64,
    // `Mutex`, not `RefCell`: `Store` has no `Sync`/`Send` requirement of
    // its own yet, but the poison idiom below (`lock().unwrap_or_else(|p|
    // p.into_inner())`, matching `rune-vfs::mem`'s convention) is what the
    // rest of this workspace already uses for shared, swappable state, so
    // WP3+ callers that DO need to touch these from another thread inherit
    // a correct pattern instead of reinventing one.
    clock: Mutex<ClockFn>,
    liveness_check: Mutex<LivenessCheckFn>,
}

impl Store {
    /// Runs the open ladder against `path` (a full file path — production
    /// callers pass `versioning::production_db_path()`; tests pass a temp
    /// path directly, so the same ladder logic is exercised either way).
    /// Returns the store plus a non-fatal degradation warning; the caller
    /// may surface the warning to the user but must not treat it as
    /// failure. `on_event` receives every writer-thread completion (plan
    /// decision 4) — `rune-tui` (WP5) adapts it into the runtime's
    /// `Sender<Msg>`. `fs` is the ONE filesystem `Probe`/`Materialize`/
    /// `Load` use (plan decision 12) — production passes
    /// `Arc::new(rune_vfs::Disk)`; tests and the fuzzer pass a shared
    /// `Arc::new(rune_vfs::Mem::new())` so the store and workspace resolve
    /// identity and disk state against the SAME files.
    pub fn open(
        path: &Path,
        fs: Arc<dyn Vfs + Send + Sync>,
        on_event: OnEvent,
    ) -> Result<(Store, Option<String>), Error> {
        let rung = open_ladder(path)?;
        let (store, warning) = Self::from_ladder(rung, fs, on_event)?;

        // Old-schema-version file GC (WP6.S3), best-effort — never blocks
        // open, mirrors the dead-session reaper's own contract. Runs
        // against `path`'s directory regardless of whether THIS open
        // degraded, since a leftover old-version file is unrelated to
        // whether today's own file could be opened.
        if let Some(parent) = path.parent() {
            crate::versioning::gc_old_versions(parent, store.liveness_check().as_ref());
        }

        Ok((store, warning))
    }

    /// Opens a store entirely in memory, undegraded (unlike the fallback
    /// rung of [`Store::open`], this is an intentional, explicit choice —
    /// tests and, eventually, the session fuzzer). `clock` seeds this
    /// store's clock from construction — a caller-supplied clock is
    /// honored even at session-establish time.
    pub fn open_in_memory(
        clock: ClockFn,
        fs: Arc<dyn Vfs + Send + Sync>,
        on_event: OnEvent,
    ) -> Result<Store, Error> {
        let uri = memory_uri();
        let mut conn = open_memory_backed(&uri)?;
        let now = clock();
        let session_id = session::establish_session(&conn, now)?;
        let liveness_check: LivenessCheckFn = Arc::new(session::is_process_alive);
        // Best-effort dead-session reaper (plan WP4.S6): never blocks open
        // — a failure here is swallowed, not surfaced, on purpose.
        let _ = crate::reaper::reap_dead_sessions(&mut conn, liveness_check.as_ref());
        // One startup blob-sweep batch (WP6.S1), after the reaper — best
        // effort, never blocks open.
        let _ = retry::with_retry(&mut conn, crate::gc::sweep_unreferenced_blobs);
        let writer = writer::spawn(conn, fs, on_event);
        let reader = reader::spawn(&uri)?;
        Ok(Store {
            writer,
            reader,
            degraded: false,
            session_id,
            next_op_id: AtomicU64::new(1),
            clock: Mutex::new(clock),
            liveness_check: Mutex::new(liveness_check),
        })
    }

    fn from_ladder(
        rung: LadderResult,
        fs: Arc<dyn Vfs + Send + Sync>,
        on_event: OnEvent,
    ) -> Result<(Store, Option<String>), Error> {
        let clock: ClockFn = Arc::new(SystemTime::now);
        let now = clock();
        let mut writer_conn = rung.writer_conn;
        let session_id = session::establish_session(&writer_conn, now)?;

        // Best-effort dead-session reaper (plan WP4.S6): never blocks open
        // — a failure here is swallowed, not surfaced, on purpose.
        let liveness_check: LivenessCheckFn = Arc::new(session::is_process_alive);
        let _ = crate::reaper::reap_dead_sessions(&mut writer_conn, liveness_check.as_ref());
        // One startup blob-sweep batch (WP6.S1), after the reaper — best
        // effort, never blocks open.
        let _ = retry::with_retry(&mut writer_conn, crate::gc::sweep_unreferenced_blobs);

        let writer = writer::spawn(writer_conn, fs, on_event);
        let reader = reader::spawn(&rung.reader_target)?;

        let store = Store {
            writer,
            reader,
            degraded: rung.degraded,
            session_id,
            next_op_id: AtomicU64::new(1),
            clock: Mutex::new(clock),
            liveness_check: Mutex::new(liveness_check),
        };
        Ok((store, rung.warning))
    }

    /// True only for the in-memory fallback rung taken when the real
    /// on-disk database could not be opened — never for an intentional
    /// [`Store::open_in_memory`]. Drives a persistent footer banner and a
    /// confirm gate before every materialize (WP5).
    pub fn degraded(&self) -> bool {
        self.degraded
    }

    /// This process's own row in `sessions` — established once at
    /// construction and never mutated after.
    pub fn session_id(&self) -> i64 {
        self.session_id
    }

    /// Replaces the store's clock. Used in deterministic tests.
    pub fn set_clock(&self, clock: ClockFn) {
        *self.clock.lock().unwrap_or_else(|p| p.into_inner()) = clock;
    }

    /// Replaces how this store decides whether a different session's
    /// recorded process is still alive. Consumed by WP4's cross-session
    /// inheritance decision.
    pub fn set_liveness_check(&self, check: LivenessCheckFn) {
        *self
            .liveness_check
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = check;
    }

    /// Returns the current liveness check, for callers (WP4) that need to
    /// invoke it directly.
    pub fn liveness_check(&self) -> LivenessCheckFn {
        Arc::clone(
            &self
                .liveness_check
                .lock()
                .unwrap_or_else(|p| p.into_inner()),
        )
    }

    /// Test-support hook: kills the writer thread as if it had died,
    /// without a real crash — see [`OpKind::KillWriterForTest`]'s doc
    /// comment. Every enqueue after this call observes
    /// [`Error::WriterGone`]. Not `#[cfg(test)]` — this needs to cross the
    /// crate boundary into `rune-tui`'s own integration tests.
    pub fn kill_writer_for_test(&self) -> Result<(), Error> {
        self.enqueue(OpKind::KillWriterForTest).map(|_| ())
    }

    /// Enqueues `kind` to the writer thread, returning the op id the
    /// eventual `DbEvent` will echo back. Never blocks — a wedged writer
    /// surfaces [`Error::WriterQueueFull`] immediately (plan Gotchas).
    pub fn enqueue(&self, kind: OpKind) -> Result<u64, Error> {
        let id = self.next_op_id.fetch_add(1, Ordering::Relaxed);
        self.writer.try_send(WriteOp { id, kind })?;
        Ok(id)
    }

    /// Blocking twin of [`Store::enqueue`], for the kill-writer test hook
    /// ONLY (`Store::probe_blocking_for_test`) — a full queue parks the
    /// caller instead of failing, and the send errors only when the writer
    /// thread has actually dropped its receiver, so `Err(WriterGone)` from
    /// this method is a true confirmation of writer death, with no
    /// `WriterQueueFull` ambiguity. `update` must never call this: every
    /// production enqueue path stays on `enqueue`/`try_send`.
    pub(crate) fn enqueue_blocking(&self, kind: OpKind) -> Result<u64, Error> {
        let id = self.next_op_id.fetch_add(1, Ordering::Relaxed);
        self.writer.send(WriteOp { id, kind })?;
        Ok(id)
    }

    /// A fresh sample of this store's injected clock.
    pub(crate) fn now(&self) -> SystemTime {
        (self.clock.lock().unwrap_or_else(|p| p.into_inner()))()
    }

    /// The reader handle, for display/immutable reads dispatched from a
    /// spawned `Cmd` — never from `update` directly.
    pub fn reader(&self) -> &reader::ReaderHandle {
        &self.reader
    }

    /// Deterministically drains and joins both threads. The writer's final
    /// op (enqueued by `writer::WriterHandle::shutdown`, WP6.S2) runs
    /// `PRAGMA wal_checkpoint(TRUNCATE)` when this session is the last live
    /// one, then `PRAGMA optimize`, before the thread actually exits.
    pub fn shutdown(self) {
        let Store {
            writer,
            reader,
            session_id,
            liveness_check,
            ..
        } = self;
        let liveness_check = liveness_check
            .into_inner()
            .unwrap_or_else(|p| p.into_inner());
        writer.shutdown(session_id, liveness_check);
        reader.shutdown();
    }
}

/// Sets the per-connection pragmas required on **both** the writer and
/// reader connections on every open (plan Gotchas: "only `journal_mode`
/// persists in the file" — everything else here does not).
pub(crate) fn apply_connection_pragmas(conn: &Connection) -> Result<(), Error> {
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "journal_size_limit", 67_108_864i64)?;
    conn.pragma_update(None, "wal_autocheckpoint", 1000i64)?;
    Ok(())
}

pub(crate) fn set_wal_mode_verified(conn: &Connection) -> Result<(), Error> {
    let mode: String =
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
    if mode.eq_ignore_ascii_case("wal") {
        Ok(())
    } else {
        Err(Error::WalModeUnavailable(mode))
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;
    use crate::writer::QUEUE_DEPTH;

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rune-db-store-test-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn noop_on_event() -> OnEvent {
        Box::new(|_evt| {})
    }

    fn test_vfs() -> Arc<dyn Vfs + Send + Sync> {
        Arc::new(rune_vfs::Disk)
    }

    /// Two `Store::open` calls against the SAME temp path (same process)
    /// both succeed, each establishing its own `sessions` row, and the file
    /// really is in WAL mode.
    #[test]
    fn two_opens_on_one_path_both_succeed_with_two_sessions_and_wal_mode() {
        let dir = temp_dir("two-opens");
        let path = dir.join("rune-v1.db");

        let (store_a, warn_a) = Store::open(&path, test_vfs(), noop_on_event()).expect("open a");
        assert!(warn_a.is_none());
        assert!(!store_a.degraded());

        let (store_b, warn_b) = Store::open(&path, test_vfs(), noop_on_event()).expect("open b");
        assert!(warn_b.is_none());
        assert!(!store_b.degraded());

        assert_ne!(store_a.session_id(), store_b.session_id());

        let verify = Connection::open(&path).expect("open verify connection");
        let sessions: i64 = verify
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .expect("count sessions");
        assert_eq!(sessions, 2);

        let mode: String = verify
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .expect("read journal_mode");
        assert_eq!(mode, "wal");

        store_a.shutdown();
        store_b.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A path whose parent can never be created (a plain FILE occupies the
    /// spot a directory needs to exist, which fails `mkdir_all` even for
    /// root) must degrade to an in-memory store, never return an error.
    #[test]
    fn unwritable_parent_degrades_to_in_memory_store_not_an_error() {
        let dir = temp_dir("unwritable");
        let blocker = dir.join("blocker");
        std::fs::write(&blocker, b"not a directory").expect("create blocker file");
        let path = blocker.join("subdir").join("rune-v1.db");

        let (store, warning) =
            Store::open(&path, test_vfs(), noop_on_event()).expect("open must not error");
        assert!(store.degraded());
        assert_eq!(warning.as_deref(), Some(DEGRADED_WARNING));

        // The degraded store must still be fully functional: writer and
        // reader threads are both alive.
        let id = store.enqueue(OpKind::Noop).expect("enqueue must succeed");
        assert!(id >= 1);

        store.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Coverage gap [rune-db 9]: the open ladder's rungs were tested for an
    /// unwritable parent, but never for the file existing and already being
    /// corrupt (garbage bytes, not a valid SQLite database). That must
    /// degrade to in-memory exactly like the unwritable-parent case, never
    /// return an error and never panic — `open_ladder`'s only hard failure
    /// is the final in-memory rung itself failing.
    #[test]
    fn corrupt_existing_db_file_degrades_to_in_memory_store_not_an_error() {
        let dir = temp_dir("corrupt-db");
        let path = dir.join("rune-v1.db");
        std::fs::write(&path, b"not a sqlite database, just garbage bytes")
            .expect("write corrupt file");

        let (store, warning) =
            Store::open(&path, test_vfs(), noop_on_event()).expect("open must not error");
        assert!(store.degraded());
        assert_eq!(warning.as_deref(), Some(DEGRADED_WARNING));

        // The degraded store must still be fully functional.
        let id = store.enqueue(OpKind::Noop).expect("enqueue must succeed");
        assert!(id >= 1);

        store.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_in_memory_is_never_degraded() {
        let clock: ClockFn = Arc::new(std::time::SystemTime::now);
        let store =
            Store::open_in_memory(clock, test_vfs(), noop_on_event()).expect("open in memory");
        assert!(!store.degraded());
        assert_eq!(store.session_id(), 1);
        store.shutdown();
    }

    /// Pins the `SendError -> WriterGone` mapping the kill-writer test hook
    /// relies on: once the writer thread has dequeued
    /// `OpKind::KillWriterForTest` and dropped its receiver, every
    /// subsequent blocking probe send must be woken with `Err(WriterGone)`
    /// — never `WriterQueueFull`, which would be a false confirmation of
    /// writer death.
    #[test]
    fn probe_blocking_for_test_confirms_writer_gone_via_send_error() {
        let clock: ClockFn = Arc::new(std::time::SystemTime::now);
        let store =
            Store::open_in_memory(clock, test_vfs(), noop_on_event()).expect("open in memory");

        store.kill_writer_for_test().expect("enqueue the kill op");

        // Bounded, not an unbounded spin: a blocking send returns `Ok`
        // only when the writer consumed a slot or a slot was free, so a
        // live writer FIFO-bound to the kill op can absorb at most (ops
        // queued ahead of the kill) + `QUEUE_DEPTH` probes before it must
        // have dequeued the kill op and dropped its receiver. Exhausting
        // this cap means the writer survived without ever reaching the
        // kill op (e.g. it went fatal on something queued first) — that
        // is a real failure to report loudly, not a hang.
        let max_attempts = 4 * QUEUE_DEPTH;
        for attempt in 0..=max_attempts {
            match store.probe_blocking_for_test(1) {
                Ok(_) => {
                    assert!(
                        attempt < max_attempts,
                        "writer never confirmed dead after {max_attempts} blocking \
                         probes — it should have dequeued the kill op long before this"
                    );
                }
                Err(err) => {
                    assert!(matches!(err, Error::WriterGone));
                    break;
                }
            }
        }

        store.shutdown();
    }
}
