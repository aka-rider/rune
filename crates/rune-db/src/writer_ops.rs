//! The writer thread's op vocabulary: the [`OpKind`] catalog a [`WriteOp`]
//! carries, the [`OpOutcome`] each one can produce, and the [`DbEvent`]
//! completion wrapping it — purely the data model; the queue/dispatch
//! machinery that consumes it lives with the writer thread itself.

use std::path::PathBuf;
use std::time::SystemTime;

use rune_core::buffer::AppliedEdit;
use rune_core::cursor::Cursor;
use rune_vfs::Stat;

use crate::ids::{DocId, ObsId, SessionId};
use crate::load::LoadResult;
use crate::materialize::{MatResult, MaterializeOutcome, MaterializePrep, MaterializeTarget};
use crate::merge_prep::MergePrepResult;
use crate::observation::Observation;
use crate::rename::RenameOutcome;
use crate::store::LivenessCheckFn;
use crate::sync::SyncState;

/// Bounded writer-queue depth. At per-keystroke-batch
/// granularity this is many seconds of furious typing; overflow implies a
/// wedged writer, which is exactly when the degraded path should trigger.
pub const QUEUE_DEPTH: usize = 1024;

/// [`OpKind::Load`]'s own read: either the writer thread performs the ONE
/// disk read itself (`Fresh`), or it adopts a `Sighting` the caller already
/// took before enqueuing (`Taken`) — an enum rather than an
/// `Option<Sighting>`-plus-a-separate-flag, so "read fresh" and "adopt this
/// sighting" stay the only two representable states.
pub enum LoadSource {
    Fresh,
    Taken(rune_vfs::Sighting),
}

/// The write operations the writer thread knows how to execute.
/// [`OpKind::Noop`] is a real op that exercises the full `BEGIN
/// IMMEDIATE`-plus-retry chokepoint without any domain semantics; the
/// journal/snapshot domain verbs sit alongside it — no table-level CRUD
/// escapes this crate, each variant below is one hand-written transaction
/// embodying its own invariant.
/// `session_id`/`now` are baked into each variant's payload
/// by the `Store` convenience method that constructs it —
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
    TestBlock(std::sync::mpsc::Receiver<()>),
    /// Test-only: deliberately panics `execute_op`, for proving the writer
    /// loop's panic guard survives a REAL unwind from op
    /// execution and that `WriterHandle::shutdown` afterward completes
    /// without hanging (the park-forever design it replaces would have
    /// deadlocked `shutdown`'s `thread.join()` here).
    #[cfg(test)]
    PanicForTest,
    /// Test-support hook (mirrors `rune_vfs::Mem::fail_next`'s permanently-
    /// public test-support surface): makes the writer thread exit its
    /// receive loop immediately, dropping its `Receiver` and thereby
    /// closing the channel from the receive side — every LATER `try_send`
    /// then observes `Error::WriterGone`, simulating the writer thread
    /// having died (a panic that somehow escaped `catch_unwind`, the
    /// process being killed) without requiring a real crash. Deliberately
    /// NOT `#[cfg(test)]`: `rune-tui`'s own integration tests (a DIFFERENT
    /// crate, where this crate's `cfg(test)` is never enabled) need this to
    /// exercise the degraded-mode banner end-to-end — gated instead behind
    /// the `test-support` feature, which `rune-tui` enables only as a
    /// dev-dependency, so it never reaches a release build.
    #[cfg(feature = "test-support")]
    KillWriterForTest,
    /// On success, the completion's `DbEvent::Ok.result` carries the
    /// journal seq of the inserted (or coalesced) event.
    AppendEdit {
        session_id: SessionId,
        now: SystemTime,
        doc_id: DocId,
        edits: Vec<AppliedEdit>,
        cursors_before: Vec<Cursor>,
        cursors_after: Vec<Cursor>,
    },
    /// `local_pos` is the enqueuing session's own LOCAL undo-journal
    /// position (`rune_core::undo::Journal::pos()` after the move) — never a
    /// pre-resolved durable seq. This writer thread resolves it to the exact
    /// durable seq itself, at execution time, using the per-doc undo-state
    /// it has been building from every `AppendEdit` it has already run for
    /// this doc (`DocUndoState`) — by the time this op reaches
    /// the front of the writer's single FIFO queue, every `AppendEdit`
    /// enqueued ahead of it has already committed, so the mapping is always
    /// exact, never a guess at an unacknowledged in-flight op the way
    /// resolving it app-side (before this rework) had to.
    MoveUndoPos {
        session_id: SessionId,
        doc_id: DocId,
        local_pos: i64,
    },
    /// On success, the completion's `DbEvent::Ok.result` carries the new
    /// `snapshots.id`. Carries no `seq` of its own — the writer resolves the
    /// anchor fresh, inside the same transaction as the insert, via
    /// `journal::current_seq` (mirrors `ResolveAdopt`'s `edit_seq: None`
    /// pattern): `content` is captured synchronously by the caller at
    /// enqueue time, and by the time this op executes every op the caller
    /// enqueued before it — including the `AppendEdit`/`MoveUndoPos` that
    /// produced `content` — has already committed, so a fresh read is exact
    /// where a caller-carried seq could only ever be a stale guess.
    CreateSnapshot {
        session_id: SessionId,
        now: SystemTime,
        doc_id: DocId,
        content: String,
    },
    /// Disk I/O (`vfs.resolve`/`stat`/`read`) happens between this op's own
    /// internal transactions, never inside one — see `probe::probe`.
    Probe {
        session_id: SessionId,
        doc_id: DocId,
        now: SystemTime,
    },
    /// Merge entry's fresh-state read — runs `probe::probe`
    /// (recording the theirs observation + blob exactly like `Probe`
    /// above) AND returns the ancestor/theirs bytes from the SAME op, so
    /// merge acts on disk state captured at one decisive moment rather
    /// than a `SyncState` alone (which carries only hashes, never bytes)
    /// plus a second, separately-timed read.
    MergePrep {
        session_id: SessionId,
        doc_id: DocId,
        now: SystemTime,
    },
    MergeOpen {
        session_id: SessionId,
        liveness_check: LivenessCheckFn,
        doc_id: DocId,
        base_obs: Option<ObsId>,
        theirs_obs: ObsId,
        marker_content: String,
        blocks_json: String,
        now: SystemTime,
    },
    MergeProgress {
        session_id: SessionId,
        liveness_check: LivenessCheckFn,
        doc_id: DocId,
        marker_content: String,
        blocks_json: String,
    },
    MergeClose {
        session_id: SessionId,
        doc_id: DocId,
        state: crate::merge_state::MergeRowState,
    },
    /// The bookkeeping-only half of `Materialize` that runs
    /// BEFORE any `vfs` call — hands the caller the decision data
    /// (`materialize::prepare_materialize`) so the actual disk publish can
    /// happen entirely off this thread, on the caller's own (`rune-tui`'s
    /// save `Cmd`).
    MaterializePrepare {
        session_id: SessionId,
        doc_id: DocId,
        target: MaterializeTarget,
    },
    /// The recording half of `Materialize`: records what the caller's own
    /// `vfs` work concluded
    /// (`materialize::record_materialize_outcome`) — the ONLY other half of
    /// `Materialize` left on this thread, and it makes no `vfs` call
    /// either. `resolved_path`/`seq` are the caller's own
    /// enqueue-time-captured facts, never re-derived here.
    MaterializeRecord {
        session_id: SessionId,
        doc_id: DocId,
        resolved_path: PathBuf,
        seq: i64,
        now: SystemTime,
        outcome: MaterializeOutcome,
    },
    /// `liveness_check` is this `Store`'s own injected liveness function
    /// (`Store::set_liveness_check`), threaded through per-op rather than
    /// read from shared state, so the writer thread never needs to touch
    /// `Store`'s mutex. `source` decides whether this op reads `path` fresh
    /// itself (`LoadSource::Fresh`, `crate::load::load`) or adopts an
    /// already-taken caller-side sighting (`LoadSource::Taken`,
    /// `crate::load::load_from_read`) — the single-sighting fix: a caller
    /// that already read `path` once must never have this op read it again.
    Load {
        session_id: SessionId,
        liveness_check: LivenessCheckFn,
        path: PathBuf,
        now: SystemTime,
        source: LoadSource,
    },
    /// Rename `from` → `to` with no clobber (`rename::rename_bind`). A
    /// collision comes back as `RenameOutcome::Collided` — a refusal, not
    /// an `Err` — carrying the destination's stat as the consent baseline
    /// for a possible [`OpKind::RenameReplace`].
    RenameFile {
        session_id: SessionId,
        doc_id: DocId,
        from: PathBuf,
        to: PathBuf,
        now: SystemTime,
    },
    /// The user-confirmed destructive rename (`rename::rename_replace`).
    /// `seen` is the stat the user consented to replace; the op re-checks
    /// it and refuses on a mismatch. Capture-then-swap-then-commit-then-
    /// unlink is deliberately ONE op: splitting it across a message
    /// boundary would make "swapped but not captured" representable.
    RenameReplace {
        session_id: SessionId,
        doc_id: DocId,
        from: PathBuf,
        to: PathBuf,
        seen: Stat,
        now: SystemTime,
    },
    /// `edit_seq: None` asks `adopt::resolve_adopt` to resolve the
    /// journal-head seq fresh, inside its own transaction, instead of
    /// trusting a value the caller could only have learned asynchronously
    /// (see that function's own doc comment) — the merge-entry flow's own
    /// case.
    ResolveAdopt {
        session_id: SessionId,
        doc_id: DocId,
        obs: ObsId,
        edit_seq: Option<i64>,
        now: SystemTime,
    },
    ResolveAbandon {
        session_id: SessionId,
        doc_id: DocId,
    },
    /// Quit-guard support: inserts a brand-new unbound scratch
    /// `documents` row. On success, the completion's `DbEvent::Ok.result`
    /// carries the new row's id.
    CreateScratch { now: SystemTime },
    /// See `scratch::gc_empty_scratch` for why this filter is
    /// stricter.
    GcEmptyScratch { keep_id: i64 },
    /// On success, the completion's `DbEvent::Ok.result` carries the
    /// candidate ids, newest first.
    RecoverableScratch { exclude_id: i64 },
    /// For an untitled document — see `scratch::reconstruct_scratch`.
    /// `liveness_check` travels with the op for the same reason `Load`
    /// carries its own copy: the writer thread never touches `Store`'s
    /// mutex.
    ReconstructScratch {
        liveness_check: LivenessCheckFn,
        doc_id: DocId,
    },
    /// Records `query` as just-used, bumping its `search_history` row's
    /// `last_used_at` (insert-or-touch — see `search_history::touch`). A
    /// cosmetic write: its own `Store` convenience method is the one place
    /// that decides an `Err` here must never sticky-degrade the store the
    /// way a failed recovery write does.
    TouchSearchQuery { query: String, now: SystemTime },
    /// The writer thread's own shutdown housekeeping —
    /// `PRAGMA wal_checkpoint(TRUNCATE)` when `session_id` is the last live
    /// session (checked FRESH via `liveness_check` against every OTHER
    /// `sessions` row — never a spawn-time snapshot, so a test's
    /// `Store::set_liveness_check` override still applies), then
    /// `PRAGMA optimize`. [`WriterHandle::shutdown`] enqueues this as the
    /// FINAL op before closing the queue, so it always runs strictly after
    /// every write already queued ahead of it.
    Shutdown {
        session_id: SessionId,
        liveness_check: LivenessCheckFn,
    },
}

/// The domain-specific result an [`OpKind`] produced, carried in
/// `DbEvent::Ok.result`. Broadened from a single `Option<i64>` as
/// `WriteOp`/`OpKind`/`Store` verbs grew, now that
/// `Probe`/`Materialize`/`Load` produce structured results richer than a
/// row id.
#[derive(Debug, Clone, PartialEq)]
pub enum OpOutcome {
    /// No meaningful return value (`Noop`, `MoveUndoPos`, `ResolveAbandon`).
    None,
    /// `AppendEdit`'s journal seq.
    Seq(crate::ids::Seq),
    /// `CreateSnapshot`'s new `snapshots.id`.
    RowId(i64),
    /// `Probe`'s resulting [`SyncState`]. Boxed: `SyncState` carries several
    /// `Option<Version>`/`String` fields, large enough that clippy's
    /// `large_enum_variant` flags the unboxed enum — the common, cheap
    /// variants (`None`/`Seq`/`RowId`) shouldn't all pay for the rare, rich
    /// ones' size.
    Sync(Box<SyncState>),
    /// `MergePrep`'s resulting [`MergePrepResult`] (boxed — see `Sync`'s
    /// doc comment).
    MergePrep(Box<MergePrepResult>),
    /// `MaterializePrepare`'s [`MaterializePrep`] — the CAS decision data
    /// the caller needs before doing any `vfs` call.
    MaterializePrep(Box<MaterializePrep>),
    /// `MaterializeRecord`'s [`MatResult`] (boxed — see `Sync`'s doc
    /// comment).
    Materialize(Box<MatResult>),
    /// `Load`'s [`LoadResult`] (boxed — see `Sync`'s doc comment).
    Load(Box<LoadResult>),
    /// `ResolveAdopt`'s resulting [`Observation`] (boxed — see `Sync`'s doc
    /// comment: the version DAG's second parent edge pushed `Observation`
    /// past the boxing threshold too).
    Observation(Box<Observation>),
    /// `RenameFile`/`RenameReplace`'s [`RenameOutcome`] (boxed — see
    /// `Sync`'s doc comment: `Replaced` carries a whole `Observation`).
    Rename(Box<RenameOutcome>),
    /// `RecoverableScratch`'s candidate `documents.id`s, newest first.
    Ids(Vec<i64>),
    /// `ReconstructScratch`'s recovered content, or `None` when there was
    /// nothing to recover (no prior session ever touched the doc, or the
    /// most recent one is still alive).
    Reconstructed(Option<String>),
}

/// A completion posted by the writer thread for one `WriteOp`, or a fatal
/// notice that the thread itself is no longer processing anything.
#[derive(Debug, Clone)]
pub enum DbEvent {
    Ok {
        id: u64,
        /// The domain-specific result the op produced (see [`OpOutcome`]).
        /// One flexible field rather than a family of `*Ok` variants,
        /// extended minimally as `WriteOp`/`OpKind` grew.
        result: OpOutcome,
    },
    Err {
        id: u64,
        error: String,
    },
    /// The writer thread caught a panic while processing `id` (if known)
    /// and has parked itself permanently — no further `WriteOp` will ever
    /// be processed. The caller must treat this exactly like a hard
    /// store failure: degrade, never retry.
    Fatal {
        error: String,
    },
}

/// Callback the writer thread delivers every [`DbEvent`] through. `Send`
/// only (not `Sync`) — owned exclusively by the writer thread, never shared.
pub type OnEvent = Box<dyn Fn(DbEvent) + Send + 'static>;
