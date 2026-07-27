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
use std::time::SystemTime;

use rusqlite::Connection;

use rune_core::buffer::AppliedEdit;
use rune_core::cursor::Cursor;

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

/// The write operations the writer thread knows how to execute. WP2 shipped
/// only [`OpKind::Noop`], a real op that exercises the full
/// `BEGIN IMMEDIATE` + retry chokepoint without any domain semantics; WP3
/// adds the journal/snapshot domain verbs (plan decision 11 — no
/// table-level CRUD escapes this crate, each variant below is one
/// hand-written transaction from `journal.rs`/`snapshot.rs` embodying its
/// own invariant). `session_id`/`now` are baked into each variant's payload
/// by the `Store` convenience method that constructs it (`store.rs`) —
/// `Store` is the one place that knows this process's session identity and
/// injected clock; the writer thread itself stays a plain
/// `Connection` executor with no identity of its own.
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
    /// Port of `journal.go:39-194` (`AppendEdit`). On success, the
    /// completion's `DbEvent::Ok.result` carries the journal seq of the
    /// inserted (or coalesced) event.
    AppendEdit {
        session_id: i64,
        now: SystemTime,
        doc_id: i64,
        edits: Vec<AppliedEdit>,
        cursors_before: Vec<Cursor>,
        cursors_after: Vec<Cursor>,
    },
    /// Port of `journal.go:293-312` (`MoveUndoPos`).
    MoveUndoPos {
        session_id: i64,
        doc_id: i64,
        pos: i64,
    },
    /// Port of `snapshot.go:74-103` (`CreateSnapshot`). On success, the
    /// completion's `DbEvent::Ok.result` carries the new `snapshots.id`.
    CreateSnapshot {
        session_id: i64,
        now: SystemTime,
        doc_id: i64,
        content: String,
        seq: i64,
    },
}

/// A completion posted by the writer thread for one [`WriteOp`], or a fatal
/// notice that the thread itself is no longer processing anything.
#[derive(Debug, Clone)]
pub enum DbEvent {
    Ok {
        id: u64,
        /// The domain-specific result the op produced, if any (e.g.
        /// `AppendEdit`'s journal seq, `CreateSnapshot`'s row id) — `None`
        /// for ops with no meaningful return value (`Noop`, `MoveUndoPos`).
        /// One flexible field rather than a family of `*Ok` variants (plan
        /// decision 4's "Ok/classified Err", extended minimally — WP3
        /// Hard rules: "extend WriteOp/OpKind as needed").
        result: Option<i64>,
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
            Ok(Ok(result)) => on_event(DbEvent::Ok { id, result }),
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

/// Runs `kind` to completion against `conn`, inside `retry::with_retry`'s
/// `BEGIN IMMEDIATE` chokepoint (plan Gotchas) for every variant that
/// touches the database. Returns the domain result (if any) that becomes
/// `DbEvent::Ok.result`.
fn execute_op(conn: &mut Connection, kind: OpKind) -> Result<Option<i64>, Error> {
    match kind {
        OpKind::Noop => {
            retry::with_retry(conn, |_tx| Ok(()))?;
            Ok(None)
        }
        #[cfg(test)]
        OpKind::TestBlock(rx) => {
            let _ = rx.recv();
            Ok(None)
        }
        OpKind::AppendEdit {
            session_id,
            now,
            doc_id,
            edits,
            cursors_before,
            cursors_after,
        } => {
            let seq = retry::with_retry(conn, |tx| {
                crate::journal::append_edit(
                    tx,
                    session_id,
                    now,
                    doc_id,
                    &edits,
                    &cursors_before,
                    &cursors_after,
                )
            })?;
            Ok(Some(seq))
        }
        OpKind::MoveUndoPos {
            session_id,
            doc_id,
            pos,
        } => {
            retry::with_retry(conn, |tx| {
                crate::journal::move_undo_pos(tx, session_id, doc_id, pos)
            })?;
            Ok(None)
        }
        OpKind::CreateSnapshot {
            session_id,
            now,
            doc_id,
            content,
            seq,
        } => {
            let row_id = retry::with_retry(conn, |tx| {
                crate::snapshot::create_snapshot(tx, session_id, now, doc_id, &content, seq)
            })?;
            Ok(Some(row_id))
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
            matches!(
                events.first(),
                Some(DbEvent::Ok {
                    id: 7,
                    result: None
                })
            ),
            "expected exactly one Ok(id: 7, result: None), got {events:?}"
        );
    }

    /// Proves `OpKind::AppendEdit` runs end-to-end through the writer
    /// thread's `BEGIN IMMEDIATE`/retry chokepoint and echoes the inserted
    /// journal seq back via `DbEvent::Ok.result` (plan Hard rule: "every
    /// write op flows through the WP2 writer FIFO/BEGIN IMMEDIATE
    /// machinery"). Domain correctness (coalescing, replay) is covered at
    /// the connection level in `journal.rs`/`tests/replay_equivalence.rs` —
    /// this test only exercises the async plumbing.
    #[test]
    fn append_edit_op_runs_through_the_writer_and_echoes_seq() {
        let conn = open_ready_connection();
        conn.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('', 'x', 'x')",
            [],
        )
        .expect("seed document");
        let doc_id = conn.last_insert_rowid();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");

        let (tx, rx) = mpsc::channel::<DbEvent>();
        let on_event: OnEvent = Box::new(move |evt| {
            let _ = tx.send(evt);
        });
        let handle = spawn(conn, on_event);

        handle
            .try_send(WriteOp {
                id: 1,
                kind: OpKind::AppendEdit {
                    session_id,
                    now: SystemTime::now(),
                    doc_id,
                    edits: vec![AppliedEdit {
                        start: 0,
                        end: 0,
                        deleted: String::new(),
                        insert: "hi".to_string(),
                    }],
                    cursors_before: vec![],
                    cursors_after: vec![],
                },
            })
            .expect("enqueue AppendEdit");

        let evt = rx.recv().expect("append edit completion");
        match evt {
            DbEvent::Ok { id: 1, result } => {
                assert_eq!(result, Some(1), "first event for this doc must be seq 1");
            }
            other => panic!("expected Ok(id:1, result:Some(seq)), got {other:?}"),
        }

        handle.shutdown();
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
