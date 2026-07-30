//! The writer thread's op vocabulary: the [`OpKind`] catalog a [`WriteOp`]
//! carries, the [`OpOutcome`] each one can produce, and the [`DbEvent`]
//! completion wrapping it. Split out of `writer.rs` (§1.6) — this module is
//! purely the data model; `writer.rs` keeps the queue/dispatch machinery
//! that consumes it.

use std::path::PathBuf;
use std::time::SystemTime;

use rune_core::buffer::AppliedEdit;
use rune_core::cursor::Cursor;
use rune_vfs::Stat;

use crate::load::LoadResult;
use crate::materialize::{MatResult, MaterializeOutcome, MaterializePrep};
use crate::observation::{ObsId, Observation};
use crate::rename::RenameOutcome;
use crate::store::LivenessCheckFn;
use crate::sync::SyncState;

/// Bounded writer-queue depth (plan Assumption A2). At per-keystroke-batch
/// granularity this is many seconds of furious typing; overflow implies a
/// wedged writer, which is exactly when the degraded path should trigger.
pub const QUEUE_DEPTH: usize = 1024;

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
    TestBlock(std::sync::mpsc::Receiver<()>),
    /// Test-only: deliberately panics `execute_op`, for proving the writer
    /// loop's panic guard (finding 2) survives a REAL unwind from op
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
    /// exercise the degraded-mode banner end-to-end (plan WP5 "Done when").
    KillWriterForTest,
    /// Port of `journal.go` (`AppendEdit`). On success, the
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
    /// Port of `journal.go` (`MoveUndoPos`).
    MoveUndoPos {
        session_id: i64,
        doc_id: i64,
        pos: i64,
    },
    /// Port of `snapshot.go` (`CreateSnapshot`). On success, the
    /// completion's `DbEvent::Ok.result` carries the new `snapshots.id`.
    CreateSnapshot {
        session_id: i64,
        now: SystemTime,
        doc_id: i64,
        content: String,
        seq: i64,
    },
    /// Port of `probe.go` (`Probe`). Disk I/O (`vfs.resolve`/`stat`/
    /// `read`) happens between this op's own internal transactions, never
    /// inside one (plan WP4.S3) — see `probe::probe`.
    Probe {
        session_id: i64,
        doc_id: i64,
        now: SystemTime,
    },
    /// WP7 step (a): the bookkeeping-only half of `Materialize` that runs
    /// BEFORE any `vfs` call — hands the caller the CAS decision data
    /// (`materialize::prepare_materialize`) so the actual disk publish can
    /// happen entirely off this thread, on the caller's own (`rune-tui`'s
    /// save `Cmd`).
    MaterializePrepare {
        doc_id: i64,
        expect: ObsId,
        bind_new: bool,
    },
    /// WP7 step (c): records what the caller's own `vfs` work concluded
    /// (`materialize::record_materialize_outcome`) — the ONLY other half of
    /// `Materialize` left on this thread, and it makes no `vfs` call
    /// either. `resolved_path`/`seq` are the caller's own
    /// enqueue-time-captured facts (§1.4.2/§1.4.8), never re-derived here.
    MaterializeRecord {
        session_id: i64,
        doc_id: i64,
        resolved_path: PathBuf,
        seq: i64,
        now: SystemTime,
        outcome: MaterializeOutcome,
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
    /// Rename `from` → `to` with no clobber (`rename::rename_bind`). A
    /// collision comes back as `RenameOutcome::Collided` — a refusal, not
    /// an `Err` — carrying the destination's stat as the consent baseline
    /// for a possible [`OpKind::RenameReplace`].
    RenameFile {
        session_id: i64,
        doc_id: i64,
        from: PathBuf,
        to: PathBuf,
        now: SystemTime,
    },
    /// The user-confirmed destructive rename (`rename::rename_replace`).
    /// `seen` is the stat the user consented to replace; the op re-checks
    /// it and refuses on a mismatch. Capture-then-swap-then-commit-then-
    /// unlink is deliberately ONE op: splitting it across a message
    /// boundary would make "swapped but not captured" representable
    /// (§1.4.10).
    RenameReplace {
        session_id: i64,
        doc_id: i64,
        from: PathBuf,
        to: PathBuf,
        seen: Stat,
        now: SystemTime,
    },
    /// Port of `adopt.go` (`ResolveAdopt`).
    ResolveAdopt {
        session_id: i64,
        doc_id: i64,
        obs: ObsId,
        edit_seq: i64,
        now: SystemTime,
    },
    /// Port of `adopt.go` (`ResolveAbandon`).
    ResolveAbandon { session_id: i64, doc_id: i64 },
    /// WP6.S2: the writer thread's own shutdown housekeeping —
    /// `PRAGMA wal_checkpoint(TRUNCATE)` when `session_id` is the last live
    /// session (checked FRESH via `liveness_check` against every OTHER
    /// `sessions` row — never a spawn-time snapshot, so a test's
    /// `Store::set_liveness_check` override still applies), then
    /// `PRAGMA optimize`. [`WriterHandle::shutdown`] enqueues this as the
    /// FINAL op before closing the queue, so it always runs strictly after
    /// every write already queued ahead of it.
    Shutdown {
        session_id: i64,
        liveness_check: LivenessCheckFn,
    },
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
    /// `MaterializePrepare`'s [`MaterializePrep`] — the CAS decision data
    /// the caller needs before doing any `vfs` call (WP7 step a).
    MaterializePrep(Box<MaterializePrep>),
    /// `MaterializeRecord`'s [`MatResult`] (boxed — see `Sync`'s doc
    /// comment) — WP7 step c.
    Materialize(Box<MatResult>),
    /// `Load`'s [`LoadResult`] (boxed — see `Sync`'s doc comment).
    Load(Box<LoadResult>),
    /// `ResolveAdopt`'s resulting [`Observation`].
    Observation(Observation),
    /// `RenameFile`/`RenameReplace`'s [`RenameOutcome`] (boxed — see
    /// `Sync`'s doc comment: `Replaced` carries a whole `Observation`).
    Rename(Box<RenameOutcome>),
}

/// A completion posted by the writer thread for one `WriteOp`, or a fatal
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
