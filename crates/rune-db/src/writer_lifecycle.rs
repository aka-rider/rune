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
#[path = "writer_lifecycle_tests.rs"]
mod tests;
