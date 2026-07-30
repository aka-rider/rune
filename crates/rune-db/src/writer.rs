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
//! channel. The loop wraps op execution, completion delivery (`on_event`
//! itself), AND idle maintenance in `catch_unwind` — a panic anywhere in
//! any of them must not vanish silently and must not corrupt an
//! in-progress transaction. It posts one best-effort [`DbEvent::Fatal`],
//! then drains every subsequently queued op with an immediate `Err` reply
//! — touching nothing but the channel and the event callback, never `conn`
//! again — until the sender side disconnects and the thread exits (see
//! `fatal`). Deliberately NOT park-forever: parking left
//! `WriterHandle::shutdown`'s `thread.join()` blocked forever, so a quit
//! after a writer panic would have hung the whole app.

use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::thread;
use std::time::Duration;

use rusqlite::Connection;

use rune_vfs::Vfs;

use crate::Error;
use crate::retry;
use crate::writer_lifecycle::{
    IDLE_TIMEOUT, fatal, run_idle_maintenance, run_shutdown_maintenance,
};
pub use crate::writer_ops::{DbEvent, OnEvent, OpKind, OpOutcome, QUEUE_DEPTH};

/// One write operation queued to the writer thread.
pub struct WriteOp {
    /// Caller-assigned id, echoed back in the eventual [`DbEvent`] so the
    /// caller can correlate completion to request (plan decision 4).
    pub id: u64,
    pub kind: OpKind,
}

/// A live handle to the writer thread: the enqueue side of its queue.
/// `sender`/`thread` are `pub(crate)` so [`WriterHandle::shutdown`]
/// (`writer_lifecycle.rs`) can destructure `self` — the shutdown sequence
/// is writer-thread lifecycle housekeeping, not queue dispatch.
pub struct WriterHandle {
    pub(crate) sender: SyncSender<WriteOp>,
    pub(crate) thread: Option<thread::JoinHandle<()>>,
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
}

/// Spawns the writer thread owning `conn`. `conn` must already have its
/// schema applied and pragmas set (`store::open`'s responsibility) — this
/// function only spawns the loop. `vfs` is the ONE filesystem every
/// disk-touching op (`Probe`/`Materialize`/`Load`) uses (plan decision 12 /
/// WP4) — owned by this thread exclusively, exactly like `conn`.
pub fn spawn(conn: Connection, vfs: Arc<dyn Vfs + Send + Sync>, on_event: OnEvent) -> WriterHandle {
    spawn_with_idle_timeout(conn, vfs, on_event, IDLE_TIMEOUT)
}

/// Like [`spawn`], but with an injectable idle timeout — the mechanism
/// WP6's idle-checkpoint/blob-sweep test uses to observe the idle path
/// firing without a multi-second test.
pub(crate) fn spawn_with_idle_timeout(
    conn: Connection,
    vfs: Arc<dyn Vfs + Send + Sync>,
    on_event: OnEvent,
    idle_timeout: Duration,
) -> WriterHandle {
    let (sender, receiver) = mpsc::sync_channel(QUEUE_DEPTH);
    let thread = thread::spawn(move || writer_loop(conn, vfs, receiver, on_event, idle_timeout));
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
    idle_timeout: Duration,
) {
    loop {
        match receiver.recv_timeout(idle_timeout) {
            Ok(op) => {
                if matches!(op.kind, OpKind::KillWriterForTest) {
                    // Drop `receiver` (by returning) rather than processing or
                    // replying — see the variant's doc comment.
                    return;
                }
                let id = op.id;
                let kind = op.kind;
                let vfs_ref = vfs.as_ref();
                // ONE guard covers executing the op AND delivering its
                // completion: a panic inside `on_event` (which runs on
                // EVERY delivery, not just a failure) is exactly as
                // dangerous as one inside `execute_op` — both must trip the
                // same fatal path, never unwind the thread silently.
                let delivered = panic::catch_unwind(AssertUnwindSafe(|| {
                    let outcome = execute_op(&mut conn, vfs_ref, kind);
                    match outcome {
                        Ok(result) => on_event(DbEvent::Ok { id, result }),
                        Err(e) => on_event(DbEvent::Err {
                            id,
                            error: e.to_string(),
                        }),
                    }
                }));
                if delivered.is_err() {
                    return fatal(receiver, on_event, format!("op {id}"));
                }
            }
            // A quiet period (plan WP6.S1): opportunistic PASSIVE checkpoint
            // plus one bounded blob-sweep batch. Best-effort — there is no
            // caller in flight to surface a failure to. Guarded exactly like
            // op processing: `run_idle_maintenance` runs on every quiet
            // period this thread will ever see, with no caller to catch an
            // unwind for it otherwise.
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let maintained =
                    panic::catch_unwind(AssertUnwindSafe(|| run_idle_maintenance(&mut conn)));
                if maintained.is_err() {
                    return fatal(receiver, on_event, "idle maintenance".to_string());
                }
            }
            // `WriterHandle::shutdown` dropped the sender after enqueueing
            // `OpKind::Shutdown` (already processed via the `Ok(op)` arm
            // above by the time this fires) — nothing left to do but exit,
            // which drops `conn` and closes it.
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
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
        // Test-only, deliberate — see the variant's doc comment. `panic!`
        // is denied workspace-wide EXCEPT in test code; this arm only ever
        // compiles under `cfg(test)`, i.e. never into the production
        // binary.
        #[cfg(test)]
        #[allow(clippy::panic)]
        OpKind::PanicForTest => panic!("intentional test panic (writer panic-guard test)"),
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
        OpKind::MaterializePrepare {
            doc_id,
            expect,
            bind_new,
        } => {
            let prep = crate::materialize::prepare_materialize(conn, doc_id, expect, bind_new)?;
            Ok(OpOutcome::MaterializePrep(Box::new(prep)))
        }
        OpKind::MaterializeRecord {
            session_id,
            doc_id,
            resolved_path,
            seq,
            now,
            outcome,
        } => {
            let result = crate::materialize::record_materialize_outcome(
                conn,
                crate::materialize::DocSession { doc_id, session_id },
                &resolved_path,
                seq,
                now,
                outcome,
            )?;
            Ok(OpOutcome::Materialize(Box::new(result)))
        }
        OpKind::RenameFile {
            session_id,
            doc_id,
            from,
            to,
            now,
        } => {
            let outcome = crate::rename_bind::rename_bind(
                conn,
                vfs,
                crate::materialize::DocSession { doc_id, session_id },
                &from,
                &to,
                now,
            )?;
            Ok(OpOutcome::Rename(Box::new(outcome)))
        }
        OpKind::RenameReplace {
            session_id,
            doc_id,
            from,
            to,
            seen,
            now,
        } => {
            let outcome = crate::rename_replace::rename_replace(
                conn,
                vfs,
                crate::materialize::DocSession { doc_id, session_id },
                &from,
                &to,
                seen,
                now,
            )?;
            Ok(OpOutcome::Rename(Box::new(outcome)))
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
        OpKind::Shutdown {
            session_id,
            liveness_check,
        } => {
            run_shutdown_maintenance(conn, session_id, liveness_check.as_ref());
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
    use std::time::SystemTime;

    use rune_core::buffer::AppliedEdit;

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
        // `shutdown` also enqueues its own `OpKind::Shutdown` housekeeping
        // op (WP6.S2), which posts a second `DbEvent` for id 0 — assert on
        // the noop's own id rather than the events vec's exact length.
        handle.shutdown(1, Arc::new(|_pid, _started_at| false));

        let events = events.lock().unwrap_or_else(|p| p.into_inner());
        assert!(
            events.iter().any(|e| matches!(
                e,
                DbEvent::Ok {
                    id: 7,
                    result: OpOutcome::None
                }
            )),
            "expected an Ok(id: 7, result: None) among {events:?}"
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

        handle.shutdown(session_id, Arc::new(crate::session::is_process_alive));
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
        handle.shutdown(1, Arc::new(|_pid, _started_at| false));
    }
}
