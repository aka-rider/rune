//! `Store`'s domain-verb convenience methods — thin `enqueue` wrappers over
//! each `OpKind` variant. Split out of `store.rs` as a second
//! `impl Store` block; `store.rs` keeps the handle's own lifecycle (open,
//! shutdown, clock/liveness plumbing).

use std::path::Path;

use rune_vfs::Stat;

use crate::Error;
use crate::observation::ObsId;
use crate::store::Store;
use crate::writer::OpKind;

impl Store {
    /// Enqueues an `AppendEdit` op for `doc_id`, tagged with this session's
    /// own identity and a fresh sample of this store's injected clock —
    /// every edit batch is enqueued to the DB writer thread and committed
    /// per batch. Fire-and-forget: the journal seq the write
    /// produced arrives asynchronously as `DbEvent::Ok.result` on the
    /// `on_event` callback this `Store` was constructed with; this method
    /// only returns the op id used to correlate that completion. See
    /// `journal::append_edit` for the transaction itself.
    pub fn append_edit(
        &self,
        doc_id: i64,
        edits: &[rune_core::buffer::AppliedEdit],
        cursors_before: &[rune_core::cursor::Cursor],
        cursors_after: &[rune_core::cursor::Cursor],
    ) -> Result<u64, Error> {
        let now = self.now();
        self.enqueue(OpKind::AppendEdit {
            session_id: self.session_id,
            now,
            doc_id,
            edits: edits.to_vec(),
            cursors_before: cursors_before.to_vec(),
            cursors_after: cursors_after.to_vec(),
        })
    }

    /// Enqueues a `MoveUndoPos` op committing this session's undo position
    /// for `doc_id` to LOCAL undo-journal position `local_pos` — call only
    /// after the corresponding buffer edit has already succeeded. The
    /// writer thread resolves `local_pos` to the exact durable seq itself
    /// at execution time (`OpKind::MoveUndoPos`'s own doc comment) — this
    /// method never resolves it.
    pub fn move_undo_pos(&self, doc_id: i64, local_pos: i64) -> Result<u64, Error> {
        self.enqueue(OpKind::MoveUndoPos {
            session_id: self.session_id,
            doc_id,
            local_pos,
        })
    }

    /// Enqueues a `TouchSearchQuery` op recording `query` as just-used in
    /// `search_history`. Cosmetic — see `OpKind::TouchSearchQuery`'s doc
    /// comment: the caller must treat an `Err` here as a display-only
    /// failure, never a store degradation.
    pub fn touch_search_query(&self, query: &str) -> Result<u64, Error> {
        let now = self.now();
        self.enqueue(OpKind::TouchSearchQuery {
            query: query.to_string(),
            now,
        })
    }

    /// Enqueues a `CreateSnapshot` op storing a recovery anchor for
    /// `doc_id` at this session's CURRENT durable journal position, resolved
    /// fresh by the writer thread at execution time (`OpKind::CreateSnapshot`'s
    /// own doc comment). See `snapshot::create_snapshot` for the transaction
    /// itself.
    pub fn create_snapshot(&self, doc_id: i64, content: &str) -> Result<u64, Error> {
        let now = self.now();
        self.enqueue(OpKind::CreateSnapshot {
            session_id: self.session_id,
            now,
            doc_id,
            content: content.to_string(),
        })
    }

    /// Enqueues a `Probe` op refreshing `doc_id`'s disk fact. See
    /// `probe::probe` for the transaction sequence. The resulting
    /// `SyncState` arrives asynchronously as
    /// `DbEvent::Ok.result` (`OpOutcome::Sync`).
    pub fn probe(&self, doc_id: i64) -> Result<u64, Error> {
        let now = self.now();
        self.enqueue(OpKind::Probe {
            session_id: self.session_id,
            doc_id,
            now,
        })
    }

    /// Enqueues a `MergePrep` op — merge entry's fresh-state read. The
    /// resulting `MergePrepResult` arrives asynchronously as
    /// `DbEvent::Ok.result` (`OpOutcome::MergePrep`).
    pub fn merge_prep(&self, doc_id: i64) -> Result<u64, Error> {
        let now = self.now();
        self.enqueue(OpKind::MergePrep {
            session_id: self.session_id,
            doc_id,
            now,
        })
    }

    /// The first, bookkeeping-only step of the materialize protocol
    /// (prepare / vfs write / record): enqueues `MaterializePrepare` —
    /// hands back the CAS decision data (`materialize::MaterializePrep`) the
    /// caller needs before it does any `vfs` call itself. Never touches
    /// `vfs`: a dead writer failing THIS enqueue means the caller falls
    /// back to an uncoordinated direct write (same as a document with no
    /// store binding at all) rather than being unable to save.
    pub fn materialize_prepare(
        &self,
        doc_id: i64,
        expect: ObsId,
        bind_new: bool,
    ) -> Result<u64, Error> {
        self.enqueue(OpKind::MaterializePrepare {
            doc_id,
            expect,
            bind_new,
        })
    }

    /// The final, recording step of the materialize protocol (prepare /
    /// vfs write / record): enqueues `MaterializeRecord`, recording what
    /// the caller's own `vfs` work (the prepare/write steps, performed
    /// entirely on the caller's thread through its OWN `Vfs` handle)
    /// concluded. `resolved_path`/`seq` are the caller's own
    /// enqueue-time-captured facts, never re-derived once this op runs. A
    /// dead writer failing THIS enqueue means the disk publish already
    /// physically completed — only this session's CAS bookkeeping is lost,
    /// which degrades the store, never the save.
    pub fn materialize_record(
        &self,
        doc_id: i64,
        resolved_path: &Path,
        seq: i64,
        outcome: crate::materialize::MaterializeOutcome,
    ) -> Result<u64, Error> {
        let now = self.now();
        self.enqueue(OpKind::MaterializeRecord {
            session_id: self.session_id,
            doc_id,
            resolved_path: resolved_path.to_path_buf(),
            seq,
            now,
            outcome,
        })
    }

    /// Enqueues a `RenameFile` op moving `doc_id`'s file from `from` to
    /// `to` without clobbering anything. A collision arrives as
    /// `OpOutcome::Rename(RenameOutcome::Collided)` — a refusal the caller
    /// turns into a guard prompt, not an error.
    pub fn rename_file(&self, doc_id: i64, from: &Path, to: &Path) -> Result<u64, Error> {
        let now = self.now();
        self.enqueue(OpKind::RenameFile {
            session_id: self.session_id,
            doc_id,
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            now,
        })
    }

    /// Enqueues a `RenameReplace` op — the user-confirmed destructive
    /// rename. `seen` is the stat the user consented to replace, captured
    /// from the preceding `Collided` outcome and re-checked inside the op
    /// (a consent check; the safety mechanism is the post-swap capture).
    pub fn rename_replace(
        &self,
        doc_id: i64,
        from: &Path,
        to: &Path,
        seen: Stat,
    ) -> Result<u64, Error> {
        let now = self.now();
        self.enqueue(OpKind::RenameReplace {
            session_id: self.session_id,
            doc_id,
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            seen,
            now,
        })
    }

    /// Enqueues a `Load` op reading `path` fresh from disk. This `Store`'s
    /// currently-installed liveness check (`set_liveness_check`) travels
    /// with the op so the writer thread never needs to touch `Store`'s own
    /// mutex.
    pub fn load(&self, path: &Path) -> Result<u64, Error> {
        let now = self.now();
        let liveness_check = self.liveness_check();
        self.enqueue(OpKind::Load {
            session_id: self.session_id,
            liveness_check,
            path: path.to_path_buf(),
            now,
        })
    }

    /// Enqueues a `ResolveAdopt` op — a user-driven [D]iscard/[M]erge
    /// resolution. `edit_seq: None` asks the op to resolve the journal-head
    /// seq itself (see `adopt::resolve_adopt`'s own doc comment) — the
    /// merge-entry flow's own case, which cannot learn its install edit's
    /// durable seq synchronously.
    pub fn resolve_adopt(
        &self,
        doc_id: i64,
        obs: ObsId,
        edit_seq: Option<i64>,
    ) -> Result<u64, Error> {
        let now = self.now();
        self.enqueue(OpKind::ResolveAdopt {
            session_id: self.session_id,
            doc_id,
            obs,
            edit_seq,
            now,
        })
    }

    /// Enqueues a `ResolveAbandon` op — the Esc-abort-out-of-the-merge-
    /// resolver counterpart to `resolve_adopt`.
    pub fn resolve_abandon(&self, doc_id: i64) -> Result<u64, Error> {
        self.enqueue(OpKind::ResolveAbandon {
            session_id: self.session_id,
            doc_id,
        })
    }

    /// Enqueues a `CreateScratch` op — mints a brand-new unbound scratch
    /// `documents` row. The new row's id arrives asynchronously as
    /// `DbEvent::Ok.result` (`OpOutcome::RowId`).
    pub fn create_scratch(&self) -> Result<u64, Error> {
        let now = self.now();
        self.enqueue(OpKind::CreateScratch { now })
    }

    /// Enqueues a `GcEmptyScratch` op sweeping empty leftover scratch rows,
    /// keeping `keep_id`. Fire-and-forget housekeeping — see
    /// `scratch::gc_empty_scratch`'s doc comment for the `inode IS NULL`
    /// filter this depends on.
    pub fn gc_empty_scratch(&self, keep_id: i64) -> Result<u64, Error> {
        self.enqueue(OpKind::GcEmptyScratch { keep_id })
    }

    /// Enqueues a `RecoverableScratch` op — the candidate ids arrive
    /// asynchronously as `DbEvent::Ok.result` (`OpOutcome::Ids`).
    pub fn recoverable_scratch(&self, exclude_id: i64) -> Result<u64, Error> {
        self.enqueue(OpKind::RecoverableScratch { exclude_id })
    }

    /// Enqueues a `ReconstructScratch` op reconstructing `doc_id`'s content
    /// across a session boundary — the result arrives asynchronously as
    /// `DbEvent::Ok.result` (`OpOutcome::Reconstructed`). This `Store`'s
    /// currently-installed liveness check travels with the op, exactly like
    /// `load`'s.
    pub fn reconstruct_scratch(&self, doc_id: i64) -> Result<u64, Error> {
        let liveness_check = self.liveness_check();
        self.enqueue(OpKind::ReconstructScratch {
            liveness_check,
            doc_id,
        })
    }
}
