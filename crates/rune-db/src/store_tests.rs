//! Tests for `Store`'s own lifecycle (open/open_in_memory/shutdown,
//! clock/liveness plumbing) — split out to keep the parent under the
//! file-size ceiling, the same shape `writer_tests.rs` already uses
//! elsewhere in this crate.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
use super::*;
use crate::writer::QUEUE_DEPTH;

fn temp_dir(label: &str) -> std::path::PathBuf {
    conn::test_temp_dir(label)
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

    let verify = conn::open_raw(&path).expect("open verify connection");
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

/// Coverage gap: the open ladder's rungs were tested for an
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
    let store = Store::open_in_memory(clock, test_vfs(), noop_on_event()).expect("open in memory");
    assert!(!store.degraded());
    assert_eq!(store.session_id(), SessionId(1));
    store.shutdown();
}

#[test]
fn store_open_secures_directory_and_database_file_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_dir("perm-fresh");
    let path = dir.join("rune-v1.db");

    let (store, _warning) = Store::open(&path, test_vfs(), noop_on_event()).expect("open store");

    let dir_mode = std::fs::metadata(&dir)
        .expect("stat dir")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700);

    let file_mode = std::fs::metadata(&path)
        .expect("stat db file")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(file_mode, 0o600);

    store.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn store_open_tightens_a_preexisting_world_readable_store_directory() {
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_dir("perm-repair-dir");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).expect("loosen dir");
    let path = dir.join("rune-v1.db");

    let (store, _warning) = Store::open(&path, test_vfs(), noop_on_event()).expect("open store");

    let dir_mode = std::fs::metadata(&dir)
        .expect("stat dir")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700);

    store.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
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
    let store = Store::open_in_memory(clock, test_vfs(), noop_on_event()).expect("open in memory");

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
        match store.probe_blocking_for_test(DocId(1)) {
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

/// `set_clock` must actually replace the installed clock, not just be a
/// well-typed no-op — the injected value must be what `Store::now`
/// reads back, immediately.
#[test]
fn set_clock_replaces_the_clock_now_reads_back() {
    let clock: ClockFn = Arc::new(std::time::SystemTime::now);
    let store = Store::open_in_memory(clock, test_vfs(), noop_on_event()).expect("open in memory");

    let fixed = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
    store.set_clock(Arc::new(move || fixed));

    assert_eq!(store.now(), fixed);
    store.shutdown();
}

/// `set_liveness_check` must actually replace the installed check —
/// `Store::liveness_check` must hand back the JUST-INSTALLED closure,
/// not the construction-time default (`session::is_process_alive`,
/// which reports a fabricated pid/start-time pair as dead).
#[test]
fn set_liveness_check_replaces_the_check_liveness_check_reads_back() {
    let clock: ClockFn = Arc::new(std::time::SystemTime::now);
    let store = Store::open_in_memory(clock, test_vfs(), noop_on_event()).expect("open in memory");

    store.set_liveness_check(Arc::new(|_pid, _started_at| true));

    assert!((store.liveness_check())(
        i64::MAX,
        "definitely not a real process"
    ));
    store.shutdown();
}

/// `shutdown` must actually drain and join the reader thread, not just
/// consume `self` and return — captured BEFORE the call, `reader_query`
/// keeps its own clone of the reader's channel sender alive regardless
/// of what happens to the `Store`'s own copy, so a query sent through it
/// AFTER `shutdown()` returns can only fail with `Error::ReaderGone` if
/// the reader thread genuinely processed its own shutdown message and
/// exited (dropping the one and only receiver) before `shutdown()`
/// returned. A no-op `shutdown` never sends that message, so the reader
/// thread is still alive and answers the query instead.
#[test]
fn shutdown_drains_and_joins_the_reader_thread() {
    let clock: ClockFn = Arc::new(std::time::SystemTime::now);
    let store = Store::open_in_memory(clock, test_vfs(), noop_on_event()).expect("open in memory");

    let reader_query = store.reader_query();
    store.shutdown();

    let result = reader_query.query(crate::reader::ReaderRequestKind::Ping);
    assert!(
        matches!(result, Err(Error::ReaderGone)),
        "expected the reader thread to already be gone, got {result:?}"
    );
}
