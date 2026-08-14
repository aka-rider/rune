//! The writer thread's own lifecycle housekeeping: the panic-guard drain
//! entered after a caught unwind, idle-period maintenance, shutdown
//! maintenance, and the WAL checkpoint primitive both call. Split out of
//! `writer.rs` — `writer.rs` keeps the queue/dispatch surface,
//! this module keeps everything that runs on a timer or on the way out.

use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc;
use std::time::Duration;

use rusqlite::Connection;

use crate::diag::background_note;
use crate::ids::SessionId;
use crate::retry;
use crate::store::LivenessCheckFn;
use crate::writer::{DbEvent, OnEvent, OpKind, WriteOp, WriterHandle};

/// How long the writer thread waits on an empty queue before treating
/// itself as idle (plan WP6.S1: "N quiet seconds (constant, e.g. 5s)").
/// Production always uses this via `writer::spawn`;
/// `writer::spawn_with_idle_timeout` lets a test install a short timeout
/// instead of actually waiting several seconds to observe the idle path
/// fire.
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
enum CheckpointMode {
    Passive,
    Truncate,
}

impl CheckpointMode {
    fn as_str(self) -> &'static str {
        match self {
            CheckpointMode::Passive => "PASSIVE",
            CheckpointMode::Truncate => "TRUNCATE",
        }
    }
}

impl WriterHandle {
    /// Drops the enqueue side and blocks until the writer thread observes
    /// disconnection and exits — a deterministic drain, never a polling
    /// loop or a wall-clock sleep. Consumes `self`: there is nothing left
    /// to enqueue to afterward.
    ///
    /// Enqueues [`OpKind::Shutdown`] first (WP6.S2) so the writer's
    /// TRUNCATE-checkpoint/`optimize` housekeeping runs strictly after every
    /// write already queued ahead of it, reading `liveness_check` FRESH at
    /// this exact moment (honors any `Store::set_liveness_check` override,
    /// mirroring `Load`'s per-op threading — WP6.S4 scenario (d) relies on
    /// this to force two real processes into a genuine TRUNCATE race).
    /// Best-effort: a full queue (a wedged writer) just skips the
    /// housekeeping — shutdown itself must never block or fail.
    pub fn shutdown(self, session_id: SessionId, liveness_check: LivenessCheckFn) {
        let WriterHandle { sender, thread } = self;
        let _ = sender.try_send(WriteOp {
            id: 0,
            kind: OpKind::Shutdown {
                session_id,
                liveness_check,
            },
        });
        drop(sender);
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }
}

/// Entered once, from exactly one place: a panic was caught somewhere in
/// the writer loop (op execution, completion delivery, or idle
/// maintenance) and `conn` is left in an unknown state that must never be
/// touched again. Posts one best-effort `DbEvent::Fatal` (itself
/// panic-guarded — a `Fatal` delivery that also panics is not this
/// function's problem to solve, only to survive), then drains every
/// subsequent queued op with an immediate `Err` reply — touching nothing
/// but the channel and the event callback — until the sender side
/// disconnects and this function (and the thread) returns.
///
/// Deliberately NOT park-forever (the prior design): parking left
/// `WriterHandle::shutdown`'s `thread.join()` blocked forever, so a quit
/// after a writer panic hung the whole app instead of exiting.
pub(crate) fn fatal(receiver: &mpsc::Receiver<WriteOp>, on_event: &OnEvent, context: &str) {
    let _ = panic::catch_unwind(AssertUnwindSafe(|| {
        on_event(DbEvent::Fatal {
            error: format!("writer thread panicked during {context}"),
        })
    }));
    while let Ok(op) = receiver.recv() {
        #[cfg(feature = "test-support")]
        if matches!(op.kind, OpKind::KillWriterForTest) {
            continue; // already fatal — nothing left to simulate killing
        }
        let _ = panic::catch_unwind(AssertUnwindSafe(|| {
            on_event(DbEvent::Err {
                id: op.id,
                error: "writer in fatal state".to_string(),
            })
        }));
    }
}

/// Runs on every quiet period (plan WP6.S1). Best-effort: a failure here is
/// exactly as harmless as a checkpoint that never got a quiet enough moment
/// to run — logged, never surfaced.
pub(crate) fn run_idle_maintenance(conn: &mut Connection) {
    if let Err(e) = checkpoint(conn, CheckpointMode::Passive) {
        background_note(&format!("idle wal_checkpoint(PASSIVE) failed: {e}"));
    }
    if let Err(e) = retry::with_retry(conn, crate::gc::sweep_unreferenced_blobs) {
        background_note(&format!("idle blob sweep failed: {e}"));
    }
}

/// Port of plan decision 9 / WP6.S2: `wal_checkpoint(TRUNCATE)` only when no
/// OTHER `sessions` row is still alive, then `PRAGMA optimize` regardless.
/// Never surfaces an error — `Store::shutdown` is infallible by design
/// (every already-acked write already committed; TRUNCATE/`optimize` are
/// pure housekeeping) — any failure is logged and swallowed, INCLUDING a
/// BUSY-class TRUNCATE failure, which is the EXPECTED outcome when two
/// sessions close at the same moment (plan Risks: "Two instances exiting
/// simultaneously both attempt TRUNCATE ... swallowed by design").
pub(crate) fn run_shutdown_maintenance(
    conn: &mut Connection,
    session_id: SessionId,
    is_alive: &dyn Fn(i64, &str) -> bool,
) {
    if is_last_live_session(conn, session_id, is_alive) {
        match checkpoint(conn, CheckpointMode::Truncate) {
            Ok(busy) if busy != 0 => {
                background_note(
                    "wal_checkpoint(TRUNCATE) could not fully complete at \
                     shutdown (busy) — expected under dual-exit, proceeding",
                );
            }
            Ok(_) => {}
            Err(e) => {
                let expected = matches!(
                    retry::classify(&e),
                    retry::Classification::RestartTransaction | retry::Classification::Backoff
                );
                if expected {
                    background_note(&format!(
                        "wal_checkpoint(TRUNCATE) busy at shutdown \
                         (expected under dual-exit): {e}"
                    ));
                } else {
                    background_note(&format!("wal_checkpoint(TRUNCATE) failed at shutdown: {e}"));
                }
            }
        }
    }
    if let Err(e) = conn.execute_batch("PRAGMA optimize") {
        background_note(&format!("PRAGMA optimize failed at shutdown: {e}"));
    }
}

/// True when no OTHER `sessions` row is currently alive per `is_alive`
/// (plan WP6.S2: "if this session is the last live one (liveness over
/// sessions rows)"). Best-effort: a query failure counts as "not last" —
/// skipping an opportunistic TRUNCATE is always safe; attempting one against
/// a `sessions` table this call couldn't even read would not be.
fn is_last_live_session(
    conn: &Connection,
    session_id: SessionId,
    is_alive: &dyn Fn(i64, &str) -> bool,
) -> bool {
    let others: Vec<(i64, String)> = match conn
        .prepare("SELECT pid, proc_started_at FROM sessions WHERE id != ?1")
        .and_then(|mut stmt| {
            let rows = stmt.query_map(rusqlite::params![session_id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
            rows.collect()
        }) {
        Ok(rows) => rows,
        Err(e) => {
            background_note(&format!(
                "shutdown: could not read sessions, skipping TRUNCATE: {e}"
            ));
            return false;
        }
    };
    !others
        .iter()
        .any(|(pid, started_at)| is_alive(*pid, started_at))
}

/// Runs `PRAGMA wal_checkpoint(<mode>)`, returning the `busy` column (1 when
/// the checkpoint could not fully complete because another connection holds
/// a conflicting lock — reported as data, not itself always a SQLite
/// error). Extra columns (`log`, `checkpointed`) are unused by any caller
/// here.
fn checkpoint(conn: &Connection, mode: CheckpointMode) -> Result<i64, rusqlite::Error> {
    conn.query_row(
        &format!("PRAGMA wal_checkpoint({})", mode.as_str()),
        [],
        |row| row.get::<_, i64>(0),
    )
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use rune_vfs::Vfs;

    use crate::writer::{OpKind, WriteOp, spawn, spawn_with_idle_timeout};

    use super::*;

    fn open_ready_connection() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory connection");
        crate::schema::apply(&conn).expect("apply schema");
        conn
    }

    fn test_vfs() -> Arc<dyn Vfs + Send + Sync> {
        Arc::new(rune_vfs::Mem::new())
    }

    /// Proves the writer idle timer actually fires (WP6.S1): with a short
    /// injected idle timeout and an empty queue, the writer's own idle
    /// maintenance sweeps an orphaned blob without any op ever being
    /// enqueued. File-backed (not `:memory:`) so a SEPARATE verify
    /// connection can observe what the writer thread wrote.
    #[test]
    fn idle_timeout_sweeps_an_orphaned_blob() {
        let dir = std::env::temp_dir().join(format!(
            "rune-db-writer-idle-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("idle-test.db");

        let conn = Connection::open(&path).expect("open file db");
        crate::schema::apply(&conn).expect("apply schema");
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
        let verify = Connection::open(&path).expect("verify connection");
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
}
