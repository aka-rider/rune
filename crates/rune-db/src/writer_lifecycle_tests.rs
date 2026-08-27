#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rune_vfs::Vfs;

use crate::writer::{OpKind, WriteOp, spawn, spawn_with_idle_timeout};

use super::*;

fn open_ready_connection() -> Connection {
    crate::conn::open_recovery_store(crate::conn::RecoveryTarget::Memory(
        &crate::conn::memory_uri(),
    ))
    .expect("open in-memory connection")
}

fn test_vfs() -> Arc<dyn Vfs + Send + Sync> {
    Arc::new(rune_vfs::Mem::new())
}

fn open_file_backed_connection(label: &str) -> (std::path::PathBuf, Connection) {
    let dir = crate::conn::test_temp_dir(label);
    let path = dir.join("rune-v1.db");
    let conn = crate::conn::open_recovery_store(crate::conn::RecoveryTarget::File(&path))
        .expect("open file-backed connection");
    (dir, conn)
}

fn grow_the_wal(conn: &Connection) {
    conn.execute_batch("CREATE TABLE t(x)")
        .expect("create scratch table");
    for i in 0..2_000i64 {
        conn.execute("INSERT INTO t(x) VALUES (?1)", rusqlite::params![i])
            .expect("insert scratch row");
    }
}

/// Proves the writer idle timer actually fires (WP6.S1): with a short
/// injected idle timeout and an empty queue, the writer's own idle
/// maintenance sweeps an orphaned blob without any op ever being
/// enqueued. File-backed (not `:memory:`) so a SEPARATE verify
/// connection can observe what the writer thread wrote.
#[test]
fn idle_timeout_sweeps_an_orphaned_blob() {
    let dir = crate::conn::test_temp_dir("writer-idle");
    let path = dir.join("idle-test.db");

    let conn = crate::conn::open_recovery_store(crate::conn::RecoveryTarget::File(&path))
        .expect("open file db");
    let hash = crate::blob::put_blob(&conn, b"orphaned").expect("seed orphaned blob");

    let handle = spawn_with_idle_timeout(
        conn,
        test_vfs(),
        Box::new(|_evt| {}),
        Duration::from_millis(20),
    );

    // Bounded poll with a deadline (not a fixed-duration pacing sleep):
    // the idle timer fires repeatedly every 20ms against an empty
    // queue, so the sweep should observe and delete the orphaned blob
    // well within the deadline.
    let verify = crate::conn::open_raw(&path).expect("verify connection");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut swept = false;
    while std::time::Instant::now() < deadline {
        let present: bool = verify
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM blobs WHERE hash=?1)",
                rusqlite::params![hash],
                |r| r.get(0),
            )
            .expect("check blob presence");
        if !present {
            swept = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(swept, "idle timer must eventually sweep the orphaned blob");

    handle.shutdown(SessionId(1), Arc::new(|_pid, _started_at| false));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Finding 2: a panic-inducing op must (a) post a `Fatal` event rather
/// than vanish silently, (b) leave the thread replying `Err` to every
/// op enqueued afterward instead of processing it against a
/// possibly-corrupt connection, and (c) — the regression this test
/// exists for — `WriterHandle::shutdown` must complete and its
/// `thread.join()` must return, never hang. The prior park-forever
/// design failed exactly (c): a quit after a writer panic would have
/// deadlocked here.
#[test]
fn panic_in_op_posts_fatal_then_shutdown_completes_without_hanging() {
    let events: Arc<Mutex<Vec<DbEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let events_for_cb = Arc::clone(&events);
    let on_event: OnEvent = Box::new(move |evt| {
        events_for_cb
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(evt);
    });

    let handle = spawn(open_ready_connection(), test_vfs(), on_event);

    handle
        .try_send(WriteOp {
            id: 1,
            kind: OpKind::PanicForTest,
        })
        .expect("enqueue the panic-inducing op");

    // Enqueued strictly after the panicking op — the FIFO ordering
    // guarantees the writer has already caught the panic and entered
    // its fatal-drain state by the time this is processed, so it must
    // observe `Err`, never be silently dropped or processed normally.
    handle
        .try_send(WriteOp {
            id: 2,
            kind: OpKind::Noop,
        })
        .expect("enqueue a follow-up op");

    // Deterministic drain: `shutdown` blocks on `thread.join()`, which
    // only returns once the writer thread's loop has actually exited —
    // this call itself is the regression assertion (it must return at
    // all, not hang).
    handle.shutdown(SessionId(1), Arc::new(|_pid, _started_at| false));

    let events = events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert!(
        events.iter().any(|e| matches!(e, DbEvent::Fatal { .. })),
        "expected a Fatal event among {events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            DbEvent::Err { id: 2, error } if error == "writer in fatal state"
        )),
        "expected op 2 to be rejected with the fatal-state error among {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, DbEvent::Ok { id: 2, .. })),
        "op 2 must never be processed against a post-panic connection: {events:?}"
    );
}

/// `checkpoint`'s `mode.as_str()` argument must land in real SQL: a
/// blank or unrecognized checkpoint-mode name silently does nothing
/// (SQLite falls back to a no-op rather than truncating), so the WAL
/// file is the one place a wrong mode name is actually observable —
/// pinning both `CheckpointMode::as_str` and `checkpoint` itself in one
/// pass, the way `checkpoint()`'s own call site (`run_shutdown_maintenance`)
/// relies on both together.
#[test]
fn checkpoint_truncate_shrinks_the_wal_file_to_near_zero() {
    let (dir, conn) = open_file_backed_connection("checkpoint-truncate");
    let wal = dir.join("rune-v1.db-wal");
    grow_the_wal(&conn);

    let wal_size_before = std::fs::metadata(&wal)
        .expect("stat wal before checkpoint")
        .len();
    assert!(
        wal_size_before > 100_000,
        "seed data must actually grow the WAL file, got {wal_size_before} bytes"
    );

    checkpoint(&conn, CheckpointMode::Truncate).expect("wal_checkpoint(TRUNCATE) must run");

    let wal_size_after = std::fs::metadata(&wal)
        .expect("stat wal after checkpoint")
        .len();
    assert_eq!(
        wal_size_after, 0,
        "TRUNCATE must shrink the WAL file to zero bytes"
    );

    drop(conn);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Port of plan decision 9 (WP6.S2): with no other `sessions` row at all,
/// this session is trivially the last live one, so shutdown must attempt
/// (and here, succeed at) the TRUNCATE checkpoint — observed the same way
/// as the test above, via the WAL file actually shrinking to zero.
#[test]
fn run_shutdown_maintenance_truncates_wal_when_no_other_session_is_alive() {
    let (dir, mut conn) = open_file_backed_connection("shutdown-truncates");
    let wal = dir.join("rune-v1.db-wal");
    grow_the_wal(&conn);

    run_shutdown_maintenance(&mut conn, SessionId(1), &|_pid, _started_at| false);

    let wal_size_after = std::fs::metadata(&wal)
        .expect("stat wal after shutdown maintenance")
        .len();
    assert_eq!(
        wal_size_after, 0,
        "the only session must TRUNCATE-checkpoint on its own shutdown"
    );

    drop(conn);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The inverse of the test above: a genuinely alive OTHER `sessions` row
/// must make this session skip the TRUNCATE attempt entirely — observed
/// by the WAL file staying exactly as large as it was left, since nothing
/// here ever calls `checkpoint` at all in that case.
#[test]
fn run_shutdown_maintenance_skips_truncate_when_another_session_is_alive() {
    let (dir, mut conn) = open_file_backed_connection("shutdown-skips");
    let wal = dir.join("rune-v1.db-wal");
    grow_the_wal(&conn);

    conn.execute(
        "INSERT INTO sessions(pid, proc_started_at, opened_at) VALUES (999999, '1.0', 'x')",
        [],
    )
    .expect("seed another session row");

    let wal_size_before = std::fs::metadata(&wal)
        .expect("stat wal before shutdown maintenance")
        .len();
    assert!(
        wal_size_before > 100_000,
        "seed data must actually grow the WAL file, got {wal_size_before} bytes"
    );

    // The seeded "other" row above is the sessions table's first insert,
    // so it claims id 1 — `session_id` must be something else, or the
    // `WHERE id != ?1` self-exclusion would exclude it too.
    run_shutdown_maintenance(&mut conn, SessionId(999), &|_pid, _started_at| true);

    let wal_size_after = std::fs::metadata(&wal)
        .expect("stat wal after shutdown maintenance")
        .len();
    assert_eq!(
        wal_size_after, wal_size_before,
        "a live other session must make shutdown skip TRUNCATE entirely"
    );

    drop(conn);
    let _ = std::fs::remove_dir_all(&dir);
}
