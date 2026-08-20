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

use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
#[cfg(feature = "test-support")]
use std::sync::mpsc::SendError;
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::thread;
use std::time::Duration;

use rusqlite::Connection;

use rune_core::assert_invariant;
use rune_vfs::Vfs;

use crate::Error;
#[cfg(test)]
use crate::ids::SessionId;
use crate::ids::{BindingToken, DocId, Seq};
use crate::writer_exec as exec;
use crate::writer_lifecycle::{IDLE_TIMEOUT, fatal, run_idle_maintenance};
pub(crate) use crate::writer_ops::OpKind;
pub use crate::writer_ops::{DbEvent, OnEvent, OpOutcome, QUEUE_DEPTH};

/// This writer thread's own record of one [`BindingToken`]'s LOCAL
/// undo-position numbering, scoped to THIS process's session (never shared
/// or persisted). A token is minted fresh by the app on every bind/rebind
/// (`rune-tui`'s `DocDb::new`), so this map starts every token over from an
/// empty entry, exactly matching a fresh binding's own `rune_core::undo::
/// Journal` starting at local position 0 — and two tokens sharing one
/// [`DocId`] (unreachable via any real open path, but not structurally
/// prevented) each get their own independent entry rather than racing to
/// fill one shared sequence. `base_seq` is the durable seq local position
/// `0` resolves to (the app's own `token_base_seq`, carried on every op that
/// might be this token's first); `local_seq[i]` is the durable seq the
/// `(i + 1)`-th `AppendEdit` THIS writer thread has run for this token
/// landed at, in the order they ran. Rebuilding this table from ops this
/// thread has ALREADY executed — rather than trusting a value carried in
/// from the app, which can only know an op's outcome once its ack has
/// round-tripped — is what makes `OpKind::MoveUndoPos`'s resolution exact
/// instead of a guess at an in-flight ack.
#[derive(Default)]
pub(crate) struct DocUndoState {
    pub(crate) base_seq: Seq,
    pub(crate) local_seq: Vec<Seq>,
}

impl DocUndoState {
    /// The durable seq LOCAL undo position `local_pos` resolves to, or
    /// `None` if this state has no entry for it — `local_pos` is deeper than
    /// any `AppendEdit` this thread has actually run for this token, an
    /// invariant violation the writer's single FIFO queue should make
    /// unreachable (see `OpKind::MoveUndoPos`'s doc comment) — never
    /// silently approximated.
    pub(crate) fn resolve(&self, local_pos: i64) -> Option<Seq> {
        if local_pos == 0 {
            return Some(self.base_seq);
        }
        let idx = usize::try_from(local_pos - 1).ok()?;
        self.local_seq.get(idx).copied()
    }

    pub(crate) fn push_seq(&mut self, doc_id: DocId, seq: Seq) {
        assert_invariant!(self.local_seq.last().is_none_or(|&last| seq > last), || {
            format!(
                "append_edit doc {doc_id}: seq {seq} did not advance past local_seq.last() {:?}",
                self.local_seq.last()
            )
        });
        self.local_seq.push(seq);
    }
}

/// Looks up `token`'s [`DocUndoState`], seeding it with `base_seq` on first
/// sight — the lazy counterpart to the old design's `Load`/`CreateScratch`-
/// time seeding: a token's numbering starts the instant its first op
/// reaches the front of the writer's queue, whichever op that is.
pub(crate) fn ensure_token_state(
    undo_state: &mut HashMap<BindingToken, DocUndoState>,
    token: BindingToken,
    base_seq: Seq,
) -> &mut DocUndoState {
    undo_state.entry(token).or_insert_with(|| DocUndoState {
        base_seq,
        local_seq: Vec::new(),
    })
}

/// One write operation queued to the writer thread.
pub(crate) struct WriteOp {
    /// Caller-assigned id, echoed back in the eventual [`DbEvent`] so the
    /// caller can correlate completion to request.
    pub id: u64,
    pub kind: OpKind,
}

/// A live handle to the writer thread: the enqueue side of its queue.
/// `sender`/`thread` are `pub(crate)` so [`WriterHandle::shutdown`]
/// (`writer_lifecycle.rs`) can destructure `self` — the shutdown sequence
/// is writer-thread lifecycle housekeeping, not queue dispatch.
pub(crate) struct WriterHandle {
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
    #[cfg(feature = "test-support")]
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
    let thread = thread::spawn(move || writer_loop(conn, &vfs, &receiver, &on_event, idle_timeout));
    WriterHandle {
        sender,
        thread: Some(thread),
    }
}

fn writer_loop(
    mut conn: Connection,
    vfs: &Arc<dyn Vfs + Send + Sync>,
    receiver: &mpsc::Receiver<WriteOp>,
    on_event: &OnEvent,
    idle_timeout: Duration,
) {
    // Owned by this thread alone, alongside `conn` — see `DocUndoState`'s
    // own doc comment.
    let mut undo_state: HashMap<BindingToken, DocUndoState> = HashMap::new();
    loop {
        match receiver.recv_timeout(idle_timeout) {
            Ok(op) => {
                #[cfg(feature = "test-support")]
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
                    let outcome = execute_op(&mut conn, vfs_ref, kind, &mut undo_state);
                    match outcome {
                        Ok(result) => on_event(DbEvent::Ok { id, result }),
                        Err(e) => on_event(DbEvent::Err {
                            id,
                            error: e.to_string(),
                        }),
                    }
                }));
                if delivered.is_err() {
                    return fatal(receiver, on_event, &format!("op {id}"));
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
                    return fatal(receiver, on_event, "idle maintenance");
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
fn execute_op(
    conn: &mut Connection,
    vfs: &dyn Vfs,
    kind: OpKind,
    undo_state: &mut HashMap<BindingToken, DocUndoState>,
) -> Result<OpOutcome, Error> {
    match kind {
        #[cfg(test)]
        OpKind::Noop => exec::noop(conn),
        #[cfg(test)]
        OpKind::TestBlock(_) | OpKind::PanicForTest => execute_test_op(kind),
        // Intercepted in `writer_loop` before this function is ever called
        // — see the variant's doc comment.
        #[cfg(feature = "test-support")]
        OpKind::KillWriterForTest => Ok(OpOutcome::None),

        // Edit ops.
        OpKind::AppendEdit {
            session_id,
            now,
            doc_id,
            edits,
            cursors_before,
            cursors_after,
            kind,
            token,
            token_base_seq,
        } => exec::append_edit(
            conn,
            undo_state,
            exec::AppendEditArgs {
                session_id,
                now,
                doc_id,
                edits,
                cursors_before,
                cursors_after,
                kind,
                token,
                token_base_seq,
            },
        ),
        OpKind::MoveUndoPos {
            session_id,
            doc_id,
            token,
            token_base_seq,
            local_pos,
        } => exec::move_undo_pos(
            conn,
            undo_state,
            session_id,
            doc_id,
            token,
            token_base_seq,
            local_pos,
        ),
        OpKind::CreateSnapshot {
            session_id,
            now,
            doc_id,
            content,
        } => exec::create_snapshot(conn, session_id, now, doc_id, content),

        // Sync/merge ops.
        OpKind::Probe {
            session_id,
            doc_id,
            now,
        } => exec::probe(conn, vfs, session_id, doc_id, now),
        OpKind::MergePrep {
            session_id,
            doc_id,
            now,
        } => exec::merge_prep(conn, vfs, session_id, doc_id, now),
        OpKind::MergeOpen {
            session_id,
            liveness_check,
            doc_id,
            base_obs,
            theirs_obs,
            marker_content,
            blocks_json,
            now,
        } => exec::merge_open(
            conn,
            exec::MergeOpenArgs {
                session_id,
                liveness_check,
                doc_id,
                base_obs,
                theirs_obs,
                marker_content,
                blocks_json,
                now,
            },
        ),
        OpKind::MergeProgress {
            session_id,
            liveness_check,
            doc_id,
            marker_content,
            blocks_json,
        } => exec::merge_progress(
            conn,
            &liveness_check,
            doc_id,
            session_id,
            &marker_content,
            &blocks_json,
        ),
        OpKind::MergeClose {
            session_id,
            doc_id,
            state,
        } => exec::merge_close(conn, session_id, doc_id, state),

        // Materialize/rename ops.
        OpKind::MaterializePrepare {
            session_id,
            doc_id,
            target,
            pending_rebaseline_hash,
        } => exec::materialize_prepare(conn, session_id, doc_id, target, pending_rebaseline_hash),
        OpKind::MaterializeRecord {
            session_id,
            doc_id,
            resolved_path,
            seq,
            now,
            outcome,
        } => exec::materialize_record(conn, session_id, doc_id, resolved_path, seq, now, outcome),
        OpKind::RenameFile {
            session_id,
            doc_id,
            from,
            to,
            now,
        } => exec::rename_file(conn, vfs, session_id, doc_id, from, to, now),
        OpKind::RenameReplace {
            session_id,
            doc_id,
            from,
            to,
            seen,
            now,
        } => exec::rename_replace(
            conn,
            vfs,
            exec::RenameReplaceArgs {
                session_id,
                doc_id,
                from,
                to,
                seen,
                now,
            },
        ),

        // Document-lifecycle ops.
        OpKind::Load {
            session_id,
            liveness_check,
            path,
            now,
            source,
        } => exec::load(
            conn,
            vfs,
            exec::LoadArgs {
                session_id,
                liveness_check,
                path,
                now,
                source,
            },
        ),
        OpKind::ResolveAdopt {
            session_id,
            doc_id,
            obs,
            edit_seq,
            now,
        } => exec::resolve_adopt(conn, session_id, doc_id, obs, edit_seq, now),
        OpKind::ResolveAbandon { session_id, doc_id } => {
            exec::resolve_abandon(conn, session_id, doc_id)
        }
        OpKind::CreateScratch {
            session_id,
            now,
            intended_path,
        } => exec::create_scratch(conn, session_id, now, intended_path),
        OpKind::GcEmptyScratch {
            keep_id,
            liveness_check,
        } => exec::gc_empty_scratch(conn, keep_id, &liveness_check),
        OpKind::RecoverableScratch { exclude_id } => exec::recoverable_scratch(conn, exclude_id),
        OpKind::FindNamedScratch { intended_path } => {
            exec::find_named_scratch(conn, &intended_path)
        }
        OpKind::ReconstructScratch {
            liveness_check,
            doc_id,
        } => exec::reconstruct_scratch(conn, &liveness_check, doc_id),
        OpKind::TouchSearchQuery { query, now } => exec::touch_search_query(conn, &query, now),
        OpKind::TouchCommandName { name, now } => exec::touch_command_name(conn, &name, now),
        OpKind::Shutdown {
            session_id,
            liveness_check,
        } => exec::shutdown(conn, session_id, &liveness_check),
    }
}

/// The `#[cfg(test)]`-only arms, moved out of the production match body
/// above: `TestBlock` rendezvous-blocks the writer thread for the bounded-
/// queue-overflow test; `PanicForTest` deliberately unwinds, proving the
/// writer loop's panic guard survives a REAL panic from op execution.
#[cfg(test)]
#[allow(clippy::panic)]
fn execute_test_op(kind: OpKind) -> Result<OpOutcome, Error> {
    match kind {
        OpKind::TestBlock(rx) => {
            let _ = rx.recv();
            Ok(OpOutcome::None)
        }
        OpKind::PanicForTest => panic!("intentional test panic (writer panic-guard test)"),
        _ => Ok(OpOutcome::None),
    }
}

#[cfg(test)]
#[path = "writer_tests.rs"]
mod tests;
