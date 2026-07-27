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
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::thread;
use std::time::SystemTime;

use rusqlite::Connection;

use rune_core::buffer::AppliedEdit;
use rune_core::cursor::Cursor;
use rune_vfs::Vfs;

use crate::Error;
use crate::load::LoadResult;
use crate::materialize::MatResult;
use crate::observation::{ObsId, Observation};
use crate::retry;
use crate::store::LivenessCheckFn;
use crate::sync::SyncState;

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
    /// Test-support hook (mirrors `rune_vfs::Mem::fail_next`'s permanently-
    /// public test-support surface): makes the writer thread exit its
    /// receive loop immediately, dropping its `Receiver` and thereby
    /// closing the channel from the receive side — every LATER `try_send`
    /// then observes `Error::WriterGone`, simulating the writer thread
    /// having died (a panic that somehow escaped `catch_unwind`, the
    /// process being killed) without requiring a real crash. Deliberately
    /// NOT `#[cfg(test)]`: `rune-tui`'s own integration tests (a DIFFERENT
    /// crate, where this crate's `cfg(test)` is never enabled) need this to
    /// exercise the degraded-mode banner end-to-end (plan WP5 "Done when").
    KillWriterForTest,
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
    /// Port of `probe.go:38-102` (`Probe`). Disk I/O (`vfs.resolve`/`stat`/
    /// `read`) happens between this op's own internal transactions, never
    /// inside one (plan WP4.S3) — see `probe::probe`.
    Probe {
        session_id: i64,
        doc_id: i64,
        now: SystemTime,
    },
    /// Port of `materialize.go` (`Materialize`) — the CAS write protocol.
    /// `content`/`expect`/`seq` are caller-captured at enqueue time
    /// (`Store::materialize`), never re-derived inside (plan WP4.S4).
    #[allow(clippy::upper_case_acronyms)]
    Materialize {
        session_id: i64,
        doc_id: i64,
        path: PathBuf,
        content: String,
        expect: ObsId,
        seq: i64,
        bind_new: bool,
        now: SystemTime,
    },
    /// Port of `load.go` (`Load`). `liveness_check` is this `Store`'s own
    /// injected liveness function (`Store::set_liveness_check`), threaded
    /// through per-op rather than read from shared state, so the writer
    /// thread never needs to touch `Store`'s mutex.
    Load {
        session_id: i64,
        liveness_check: LivenessCheckFn,
        path: PathBuf,
        now: SystemTime,
    },
    /// Port of `adopt.go:9-31` (`ResolveAdopt`).
    ResolveAdopt {
        session_id: i64,
        doc_id: i64,
        obs: ObsId,
        edit_seq: i64,
        now: SystemTime,
    },
    /// Port of `adopt.go:33-99` (`ResolveAbandon`).
    ResolveAbandon { session_id: i64, doc_id: i64 },
}

/// The domain-specific result an [`OpKind`] produced, carried in
/// `DbEvent::Ok.result`. Broadened from WP2/WP3's single `Option<i64>`
/// (plan WP4 Hard rules: "extend WriteOp/OpKind + Store verbs") now that
/// `Probe`/`Materialize`/`Load` produce structured results richer than a
/// row id.
#[derive(Debug, Clone, PartialEq)]
pub enum OpOutcome {
    /// No meaningful return value (`Noop`, `MoveUndoPos`, `ResolveAbandon`).
    None,
    /// `AppendEdit`'s journal seq.
    Seq(i64),
    /// `CreateSnapshot`'s new `snapshots.id`.
    RowId(i64),
    /// `Probe`'s resulting [`SyncState`]. Boxed: `SyncState` carries several
    /// `Option<Version>`/`String` fields, large enough that clippy's
    /// `large_enum_variant` flags the unboxed enum — the common, cheap
    /// variants (`None`/`Seq`/`RowId`) shouldn't all pay for the rare, rich
    /// ones' size.
    Sync(Box<SyncState>),
    /// `Materialize`'s [`MatResult`] (boxed — see `Sync`'s doc comment).
    Materialize(Box<MatResult>),
    /// `Load`'s [`LoadResult`] (boxed — see `Sync`'s doc comment).
    Load(Box<LoadResult>),
    /// `ResolveAdopt`'s resulting [`Observation`].
    Observation(Observation),
}

/// A completion posted by the writer thread for one [`WriteOp`], or a fatal
/// notice that the thread itself is no longer processing anything.
#[derive(Debug, Clone)]
pub enum DbEvent {
    Ok {
        id: u64,
        /// The domain-specific result the op produced (see [`OpOutcome`]).
        /// One flexible field rather than a family of `*Ok` variants (plan
        /// decision 4's "Ok/classified Err", extended minimally — WP3/WP4
        /// Hard rules: "extend WriteOp/OpKind as needed").
        result: OpOutcome,
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
/// function only spawns the loop. `vfs` is the ONE filesystem every
/// disk-touching op (`Probe`/`Materialize`/`Load`) uses (plan decision 12 /
/// WP4) — owned by this thread exclusively, exactly like `conn`.
pub fn spawn(conn: Connection, vfs: Arc<dyn Vfs + Send + Sync>, on_event: OnEvent) -> WriterHandle {
    let (sender, receiver) = mpsc::sync_channel(QUEUE_DEPTH);
    let thread = thread::spawn(move || writer_loop(conn, vfs, receiver, on_event));
    WriterHandle {
        sender,
        thread: Some(thread),
    }
}

fn writer_loop(
    mut conn: Connection,
    vfs: Arc<dyn Vfs + Send + Sync>,
    receiver: mpsc::Receiver<WriteOp>,
    on_event: OnEvent,
) {
    while let Ok(op) = receiver.recv() {
        if matches!(op.kind, OpKind::KillWriterForTest) {
            // Drop `receiver` (by returning) rather than processing or
            // replying — see the variant's doc comment.
            return;
        }
        let id = op.id;
        let kind = op.kind;
        let vfs_ref = vfs.as_ref();
        let outcome =
            panic::catch_unwind(AssertUnwindSafe(|| execute_op(&mut conn, vfs_ref, kind)));
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
/// `DbEvent::Ok.result`. `Probe`/`Materialize`/`Load` call several
/// `retry::with_retry` transactions internally, interleaved with `vfs`
/// calls made with NO transaction open (plan binding rule / Go invariant
/// I1) — `execute_op` itself never wraps their whole body in one tx.
fn execute_op(conn: &mut Connection, vfs: &dyn Vfs, kind: OpKind) -> Result<OpOutcome, Error> {
    match kind {
        OpKind::Noop => {
            retry::with_retry(conn, |_tx| Ok(()))?;
            Ok(OpOutcome::None)
        }
        #[cfg(test)]
        OpKind::TestBlock(rx) => {
            let _ = rx.recv();
            Ok(OpOutcome::None)
        }
        // Intercepted in `writer_loop` before this function is ever called
        // — see the variant's doc comment.
        OpKind::KillWriterForTest => Ok(OpOutcome::None),
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
            Ok(OpOutcome::Seq(seq))
        }
        OpKind::MoveUndoPos {
            session_id,
            doc_id,
            pos,
        } => {
            retry::with_retry(conn, |tx| {
                crate::journal::move_undo_pos(tx, session_id, doc_id, pos)
            })?;
            Ok(OpOutcome::None)
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
            Ok(OpOutcome::RowId(row_id))
        }
        OpKind::Probe {
            session_id,
            doc_id,
            now,
        } => {
            let state = crate::probe::probe(conn, vfs, session_id, doc_id, now)?;
            Ok(OpOutcome::Sync(Box::new(state)))
        }
        OpKind::Materialize {
            session_id,
            doc_id,
            path,
            content,
            expect,
            seq,
            bind_new,
            now,
        } => {
            let result = crate::materialize::materialize(
                conn, vfs, session_id, doc_id, &path, &content, expect, seq, bind_new, now,
            )?;
            Ok(OpOutcome::Materialize(Box::new(result)))
        }
        OpKind::Load {
            session_id,
            liveness_check,
            path,
            now,
        } => {
            let result =
                crate::load::load(conn, vfs, session_id, liveness_check.as_ref(), &path, now)?;
            Ok(OpOutcome::Load(Box::new(result)))
        }
        OpKind::ResolveAdopt {
            session_id,
            doc_id,
            obs,
            edit_seq,
            now,
        } => {
            let observation =
                crate::adopt::resolve_adopt(conn, session_id, doc_id, obs, edit_seq, now)?;
            Ok(OpOutcome::Observation(observation))
        }
        OpKind::ResolveAbandon { session_id, doc_id } => {
            crate::adopt::resolve_abandon(conn, session_id, doc_id)?;
            Ok(OpOutcome::None)
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

    fn test_vfs() -> Arc<dyn Vfs + Send + Sync> {
        Arc::new(rune_vfs::Mem::new())
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

        let handle = spawn(open_ready_connection(), test_vfs(), on_event);
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
                    result: OpOutcome::None
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
        let handle = spawn(conn, test_vfs(), on_event);

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
                assert_eq!(
                    result,
                    OpOutcome::Seq(1),
                    "first event for this doc must be seq 1"
                );
            }
            other => panic!("expected Ok(id:1, result:Seq(seq)), got {other:?}"),
        }

        handle.shutdown();
    }

    #[test]
    fn stalled_writer_returns_full_without_blocking_or_panicking() {
        let (block_tx, block_rx) = mpsc::channel::<()>();
        let on_event: OnEvent = Box::new(|_evt| {});

        let handle = spawn(open_ready_connection(), test_vfs(), on_event);

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
