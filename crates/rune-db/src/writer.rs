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
use crate::ids::{DocId, Seq};
use crate::retry;
use crate::writer_lifecycle::{
    IDLE_TIMEOUT, fatal, run_idle_maintenance, run_shutdown_maintenance,
};
pub use crate::writer_ops::{DbEvent, OnEvent, OpKind, OpOutcome, QUEUE_DEPTH};

/// This writer thread's own record of one bound document's LOCAL
/// undo-position numbering, scoped to THIS process's session (never shared
/// or persisted — a fresh `writer_loop` starts every doc over from an empty
/// map, exactly matching a fresh session's own `rune_core::undo::Journal`
/// starting at position 0). `base_seq` is the durable seq local position `0`
/// resolves to (this session's durable journal head at the moment `doc_id`
/// was bound — a cross-session inheritance bridge edit if one was journaled
/// at load, else the position this session found the doc at); `local_seq[i]`
/// is the durable seq the `(i + 1)`-th `AppendEdit` THIS writer thread has
/// run for `doc_id` landed at, in the order they ran. Rebuilding this table
/// from ops this thread has ALREADY executed — rather than trusting a value
/// carried in from the app, which can only know an op's outcome once its ack
/// has round-tripped — is what makes `OpKind::MoveUndoPos`'s resolution
/// exact instead of a guess at an in-flight ack.
#[derive(Default)]
struct DocUndoState {
    base_seq: Seq,
    local_seq: Vec<Seq>,
}

impl DocUndoState {
    /// The durable seq LOCAL undo position `local_pos` resolves to, or
    /// `None` if this state has no entry for it — either `doc_id` was never
    /// bound (no `Load`/`CreateScratch` this writer thread ever ran for it)
    /// or `local_pos` is deeper than any `AppendEdit` this thread has
    /// actually run, both of which are invariant violations the writer's
    /// single FIFO queue should make unreachable (see `OpKind::MoveUndoPos`'s
    /// doc comment) — never silently approximated.
    fn resolve(&self, local_pos: i64) -> Option<Seq> {
        if local_pos == 0 {
            return Some(self.base_seq);
        }
        let idx = usize::try_from(local_pos - 1).ok()?;
        self.local_seq.get(idx).copied()
    }

    fn push_seq(&mut self, doc_id: DocId, seq: Seq) {
        assert_invariant!(self.local_seq.last().is_none_or(|&last| seq > last), || {
            format!(
                "append_edit doc {doc_id}: seq {seq} did not advance past local_seq.last() {:?}",
                self.local_seq.last()
            )
        });
        self.local_seq.push(seq);
    }
}

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
    // Owned by this thread alone, alongside `conn` — see `DocUndoState`'s
    // own doc comment.
    let mut undo_state: HashMap<DocId, DocUndoState> = HashMap::new();
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
fn execute_op(
    conn: &mut Connection,
    vfs: &dyn Vfs,
    kind: OpKind,
    undo_state: &mut HashMap<DocId, DocUndoState>,
) -> Result<OpOutcome, Error> {
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
        #[cfg(feature = "test-support")]
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
            // A real edit batch (never empty — see `db_enqueue::append_edit`'s
            // caller) always lands a genuine row, so `seq` is always > 0
            // here; recording it extends this doc's local-position mapping
            // by exactly one entry, matching the ONE local `Journal::push`
            // this `AppendEdit` replicates. With coalescing gone, every
            // `append_edit` call lands a fresh row, so `seq` is now always
            // strictly greater than the previous entry — a violation would
            // mean the journal grew a coalescing path again without
            // updating this mapping.
            let state = undo_state.entry(doc_id).or_default();
            state.push_seq(doc_id, seq);
            Ok(OpOutcome::Seq(seq))
        }
        OpKind::MoveUndoPos {
            session_id,
            doc_id,
            local_pos,
        } => {
            let target_seq = undo_state
                .get(&doc_id)
                .and_then(|state| state.resolve(local_pos))
                .ok_or_else(|| {
                    Error::NotFound(format!(
                        "move_undo_pos: doc {doc_id} has no durable seq for local position {local_pos}"
                    ))
                })?;
            retry::with_retry(conn, |tx| {
                crate::journal::move_undo_pos(tx, session_id, doc_id, target_seq)
            })?;
            Ok(OpOutcome::None)
        }
        OpKind::CreateSnapshot {
            session_id,
            now,
            doc_id,
            content,
        } => {
            let row_id = retry::with_retry(conn, |tx| {
                // Resolved fresh, inside the same transaction as the insert
                // — see `OpKind::CreateSnapshot`'s own doc comment.
                let seq = crate::journal::current_seq(tx, session_id, doc_id)?;
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
        OpKind::MergeOpen {
            session_id,
            liveness_check,
            doc_id,
            base_obs,
            theirs_obs,
            marker_content,
            blocks_json,
            now,
        } => {
            crate::merge_state::merge_open(
                conn,
                liveness_check.as_ref(),
                crate::merge_state::MergeOpenArgs {
                    doc_id,
                    session_id,
                    base_obs,
                    theirs_obs,
                    marker_content: &marker_content,
                    blocks_json: &blocks_json,
                },
                now,
            )?;
            Ok(OpOutcome::None)
        }
        OpKind::MergeProgress {
            session_id,
            liveness_check,
            doc_id,
            marker_content,
            blocks_json,
        } => {
            crate::merge_state::merge_progress(
                conn,
                liveness_check.as_ref(),
                doc_id,
                session_id,
                &marker_content,
                &blocks_json,
            )?;
            Ok(OpOutcome::None)
        }
        OpKind::MergeClose {
            session_id,
            doc_id,
            state,
        } => {
            crate::merge_state::merge_close(conn, doc_id, session_id, state)?;
            Ok(OpOutcome::None)
        }
        OpKind::MaterializePrepare {
            session_id,
            doc_id,
            target,
        } => {
            let prep = crate::materialize::prepare_materialize(
                conn,
                crate::materialize::DocSession { doc_id, session_id },
                target,
            )?;
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
            source,
        } => {
            let result = match source {
                crate::writer_ops::LoadSource::Fresh => {
                    crate::load::load(conn, vfs, session_id, liveness_check.as_ref(), &path, now)?
                }
                crate::writer_ops::LoadSource::Taken(sighting) => {
                    let read = crate::bracket::BracketedRead {
                        data: sighting.bytes,
                        stat: crate::bracket::stat_facts_from(sighting.sighted.stat()),
                        confirmed: sighting.sighted.is_confirmed(),
                    };
                    crate::load::load_from_read(
                        conn,
                        vfs,
                        session_id,
                        liveness_check.as_ref(),
                        &path,
                        read,
                        now,
                    )?
                }
            };
            // A fresh binding — this document's LOCAL undo-journal position
            // `0` (no local pushes yet this binding) durably predates
            // `bridge_seq` if this load journaled a cross-session
            // inheritance bridge edit, else it predates whatever this
            // session already found at `doc_id` (0 for a genuinely fresh
            // document). Replaces, never merges with, any stale entry a
            // PRIOR binding of this same `doc_id` left behind (a close then
            // reopen within one process resets local position numbering
            // right along with it).
            undo_state.insert(
                result.doc_id,
                DocUndoState {
                    base_seq: result.bridge_seq.unwrap_or(Seq(0)),
                    local_seq: Vec::new(),
                },
            );
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
            Ok(OpOutcome::Observation(Box::new(observation)))
        }
        OpKind::ResolveAbandon { session_id, doc_id } => {
            crate::adopt::resolve_abandon(conn, session_id, doc_id)?;
            Ok(OpOutcome::None)
        }
        OpKind::CreateScratch { now } => {
            let id = crate::scratch::create_scratch(conn, now)?;
            // A brand-new row, never bound before — local position `0`
            // starts at durable seq `0`, same as `Load`'s doc comment.
            undo_state.insert(id, DocUndoState::default());
            Ok(OpOutcome::RowId(id.0))
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
