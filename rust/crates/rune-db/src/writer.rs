//! The writer thread: owns the single read-write connection, drains a
//! bounded FIFO queue of [`WriteOp`]s, and runs every op inside
//! `BEGIN IMMEDIATE` via `retry.rs` (plan decision 7: "one writer thread
//! owning one read-write connection, FIFO queue for all stateful ops
//! (read-your-writes by construction)").
//!
//! The queue is `std::sync::mpsc::sync_channel(1024)` (plan Assumption A2).
//! Enqueue uses `try_send`: a full queue means the writer is wedged, and
//! `update` (the caller, `rune-tui`'s Elm-style loop) must never block on
//! I/O (plan Gotchas) — `TrySendError::Full` maps to an immediate
//! [`Error::WriterQueueFull`](crate::Error::WriterQueueFull) instead.
//!
//! Every completion — success or classified failure — is delivered through
//! an injected `on_event` callback (plan decision 4: "op carries a `u64` op
//! id; writer thread posts a completion ... into the runtime's existing
//! `Sender<Msg>`"); `rune-tui` (WP5) adapts it to the runtime's `Msg`
//! channel. The loop wraps each op in `catch_unwind` — a panic must not
//! vanish silently and must not corrupt an in-progress transaction; it
//! posts [`DbEvent::Fatal`] and parks the thread forever rather than
//! continuing to process ops against a connection left in an unknown state.

use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::thread;

use rusqlite::Connection;

use crate::Error;
use crate::retry;

/// Bounded writer-queue depth (plan Assumption A2). At per-keystroke-batch
/// granularity this is many seconds of furious typing; overflow implies a
/// wedged writer, which is exactly when the degraded path should trigger.
pub const QUEUE_DEPTH: usize = 1024;

/// One write operation queued to the writer thread.
pub struct WriteOp {
    /// Caller-assigned id, echoed back in the eventual [`DbEvent`] so the
    /// caller can correlate completion to request (plan decision 4).
    pub id: u64,
    pub kind: OpKind,
}

/// The write operations the writer thread knows how to execute. WP2 ships
/// only [`OpKind::Noop`] — a real op that exercises the full
/// `BEGIN IMMEDIATE` + retry chokepoint without any domain semantics yet;
/// the domain verbs (`AppendEdit`, `MoveUndoPos`, `CreateSnapshot`, ...)
/// land in WP3+, each as one hand-written transaction embodying its own
/// invariant (plan decision 11 — no table-level CRUD escapes this crate).
pub enum OpKind {
    /// Executes an empty `BEGIN IMMEDIATE` / `COMMIT` — proves the writer's
    /// execute-with-retry path end-to-end with no side effects.
    Noop,
    /// Test-only: blocks the writer thread until a signal arrives on the
    /// receiver, used to stall the writer deterministically for the
    /// bounded-queue-overflow test (no wall-clock sleeps to pace this, per
    /// repo convention — a real rendezvous instead).
    #[cfg(test)]
    TestBlock(mpsc::Receiver<()>),
}

/// A completion posted by the writer thread for one [`WriteOp`], or a fatal
/// notice that the thread itself is no longer processing anything.
#[derive(Debug, Clone)]
pub enum DbEvent {
    Ok {
        id: u64,
    },
    Err {
        id: u64,
        error: String,
    },
    /// The writer thread caught a panic while processing `id` (if known)
    /// and has parked itself permanently — no further `WriteOp` will ever
    /// be processed. The caller (WP5) must treat this exactly like a hard
    /// store failure: degrade, never retry.
    Fatal {
        error: String,
    },
}

/// Callback the writer thread delivers every [`DbEvent`] through. `Send`
/// only (not `Sync`) — owned exclusively by the writer thread, never shared.
pub type OnEvent = Box<dyn Fn(DbEvent) + Send + 'static>;

/// A live handle to the writer thread: the enqueue side of its queue.
pub struct WriterHandle {
    sender: SyncSender<WriteOp>,
    thread: Option<thread::JoinHandle<()>>,
}

impl WriterHandle {
    /// Enqueues `op`. Never blocks: a full queue maps to
    /// [`Error::WriterQueueFull`] immediately (plan Gotchas).
    pub fn try_send(&self, op: WriteOp) -> Result<(), Error> {
        self.sender.try_send(op).map_err(|e| match e {
            TrySendError::Full(_) => Error::WriterQueueFull,
            TrySendError::Disconnected(_) => Error::WriterGone,
        })
    }

    /// Drops the enqueue side and blocks until the writer thread observes
    /// disconnection and exits — a deterministic drain, never a polling
    /// loop or a wall-clock sleep. Consumes `self`: there is nothing left
    /// to enqueue to afterward.
    pub fn shutdown(self) {
        let WriterHandle { sender, thread } = self;
        drop(sender);
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }
}

/// Spawns the writer thread owning `conn`. `conn` must already have its
/// schema applied and pragmas set (`store::open`'s responsibility) — this
/// function only spawns the loop.
pub fn spawn(conn: Connection, on_event: OnEvent) -> WriterHandle {
    let (sender, receiver) = mpsc::sync_channel(QUEUE_DEPTH);
    let thread = thread::spawn(move || writer_loop(conn, receiver, on_event));
    WriterHandle {
        sender,
        thread: Some(thread),
    }
}

fn writer_loop(mut conn: Connection, receiver: mpsc::Receiver<WriteOp>, on_event: OnEvent) {
    while let Ok(op) = receiver.recv() {
        let id = op.id;
        let kind = op.kind;
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| execute_op(&mut conn, kind)));
        match outcome {
            Ok(Ok(())) => on_event(DbEvent::Ok { id }),
            Ok(Err(e)) => on_event(DbEvent::Err {
                id,
                error: e.to_string(),
            }),
            Err(_) => {
                on_event(DbEvent::Fatal {
                    error: format!("writer thread panicked processing op {id}"),
                });
                // Never process another op against a connection left in an
                // unknown state after an unwind — park forever rather than
                // exit, so the thread's presence (and the queue behind it)
                // stays diagnosable rather than silently vanishing.
                loop {
                    thread::park();
                }
            }
        }
    }
}

fn execute_op(conn: &mut Connection, kind: OpKind) -> Result<(), Error> {
    match kind {
        OpKind::Noop => retry::with_retry(conn, |_tx| Ok(())),
        #[cfg(test)]
        OpKind::TestBlock(rx) => {
            let _ = rx.recv();
            Ok(())
        }
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
    use std::sync::{Arc, Mutex};

    fn open_ready_connection() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory connection");
        crate::schema::apply(&conn).expect("apply schema");
        conn
    }

    #[test]
    fn noop_op_round_trips_ok() {
        let events: Arc<Mutex<Vec<DbEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let events_for_cb = Arc::clone(&events);
        let on_event: OnEvent = Box::new(move |evt| {
            events_for_cb
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(evt);
        });

        let handle = spawn(open_ready_connection(), on_event);
        handle
            .try_send(WriteOp {
                id: 7,
                kind: OpKind::Noop,
            })
            .expect("enqueue noop");

        // Dropping the sender (inside shutdown) closes the queue once
        // drained, so `join` returns only after the writer thread's `recv`
        // loop has processed our op and exited — deterministic, no polling.
        handle.shutdown();

        let events = events.lock().unwrap_or_else(|p| p.into_inner());
        assert_eq!(events.len(), 1);
        assert!(
            matches!(events.first(), Some(DbEvent::Ok { id: 7 })),
            "expected exactly one Ok(id: 7), got {events:?}"
        );
    }

    #[test]
    fn stalled_writer_returns_full_without_blocking_or_panicking() {
        let (block_tx, block_rx) = mpsc::channel::<()>();
        let on_event: OnEvent = Box::new(|_evt| {});

        let handle = spawn(open_ready_connection(), on_event);

        // The first op stalls the writer thread indefinitely until we
        // signal it — a deterministic rendezvous, not a sleep.
        handle
            .try_send(WriteOp {
                id: 0,
                kind: OpKind::TestBlock(block_rx),
            })
            .expect("enqueue the stalling op");

        let mut saw_full = false;
        for i in 1..=(QUEUE_DEPTH as u64 + 8) {
            match handle.try_send(WriteOp {
                id: i,
                kind: OpKind::Noop,
            }) {
                Ok(()) => {}
                Err(Error::WriterQueueFull) => {
                    saw_full = true;
                    break;
                }
                Err(other) => panic!("unexpected error enqueueing: {other}"),
            }
        }
        assert!(
            saw_full,
            "a stalled writer with a bounded queue must eventually return Full"
        );

        // Unblock the writer so it can drain the rest of the queue and the
        // thread can exit cleanly during shutdown.
        let _ = block_tx.send(());
        drop(block_tx);
        handle.shutdown();
    }
}
