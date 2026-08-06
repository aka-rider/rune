//! The writer thread: owns the single read-write connection, drains a
//! bounded FIFO queue of [`WriteOp`]s, and runs every op inside
//! `BEGIN IMMEDIATE` via `retry.rs`. One writer thread owns one read-write
//! connection, with a FIFO queue for all stateful ops, giving
//! read-your-writes by construction.
//!
//! The queue is `std::sync::mpsc::sync_channel(1024)`. Enqueue uses
//! `try_send`: a full queue means the writer is wedged, and `update` (the
//! caller, `rune-tui`'s Elm-style loop) must never block on I/O —
//! `TrySendError::Full` maps to an immediate
//! [`Error::WriterQueueFull`](crate::Error::WriterQueueFull) instead.
//!
//! Every completion — success or classified failure — is delivered through
//! an injected `on_event` callback: each op carries a `u64` op id, and the
//! writer thread posts a completion into the runtime's existing
//! `Sender<Msg>`; `rune-tui` adapts it to the runtime's `Msg`
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
use std::sync::mpsc::{self, SendError, SyncSender, TrySendError};
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
    /// caller can correlate completion to request.
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
    /// [`Error::WriterQueueFull`] immediately.
    pub fn try_send(&self, op: WriteOp) -> Result<(), Error> {
        self.sender.try_send(op).map_err(|e| match e {
            TrySendError::Full(_) => Error::WriterQueueFull,
            TrySendError::Disconnected(_) => Error::WriterGone,
        })
    }

    /// Blocking counterpart to [`WriterHandle::try_send`], for the
    /// kill-writer test hook ONLY — production enqueue must never block
    /// `update` on a full queue, so every production path stays on
    /// `try_send`. A full queue parks the caller until the writer frees a
    /// slot; the send is woken with `Err` the moment the writer thread
    /// drops its receiver, i.e. the error IS the writer-death signal —
    /// there is no full-queue error case at all.
    pub(crate) fn send(&self, op: WriteOp) -> Result<(), Error> {
        self.sender
            .send(op)
            .map_err(|SendError(_)| Error::WriterGone)
    }
}

/// Spawns the writer thread owning `conn`. `conn` must already have its
/// schema applied and pragmas set (`store::open`'s responsibility) — this
/// function only spawns the loop. `vfs` is the ONE filesystem every
/// disk-touching op (`Probe`/`Materialize`/`Load`) uses — owned by this
/// thread exclusively, exactly like `conn`.
pub fn spawn(conn: Connection, vfs: Arc<dyn Vfs + Send + Sync>, on_event: OnEvent) -> WriterHandle {
    spawn_with_idle_timeout(conn, vfs, on_event, IDLE_TIMEOUT)
}

/// Like [`spawn`], but with an injectable idle timeout — the mechanism the
/// idle-checkpoint/blob-sweep test uses to observe the idle path firing
/// without a multi-second test.
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
            // A quiet period: opportunistic PASSIVE checkpoint
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
/// `BEGIN IMMEDIATE` chokepoint for every variant that
/// touches the database. Returns the domain result (if any) that becomes
/// `DbEvent::Ok.result`. `Probe`/`Materialize`/`Load` call several
/// `retry::with_retry` transactions internally, interleaved with `vfs`
/// calls made with NO transaction open (plan binding rule / invariant
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
        OpKind::MergePrep {
            session_id,
            doc_id,
            now,
        } => {
            let result = crate::merge_prep::merge_prep(conn, vfs, session_id, doc_id, now)?;
            Ok(OpOutcome::MergePrep(Box::new(result)))
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
        OpKind::CreateScratch { now } => {
            let id = crate::scratch::create_scratch(conn, now)?;
            Ok(OpOutcome::RowId(id))
        }
        OpKind::GcEmptyScratch { keep_id } => {
            crate::scratch::gc_empty_scratch(conn, keep_id)?;
            Ok(OpOutcome::None)
        }
        OpKind::RecoverableScratch { exclude_id } => {
            let ids = crate::scratch::recoverable_scratch(conn, exclude_id)?;
            Ok(OpOutcome::Ids(ids))
        }
        OpKind::ReconstructScratch {
            liveness_check,
            doc_id,
        } => {
            let content =
                crate::scratch::reconstruct_scratch(conn, liveness_check.as_ref(), doc_id)?;
            Ok(OpOutcome::Reconstructed(content))
        }
        OpKind::TouchSearchQuery { query, now } => {
            retry::with_retry(conn, |tx| crate::search_history::touch(tx, &query, now))?;
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
#[path = "writer_tests.rs"]
mod tests;
