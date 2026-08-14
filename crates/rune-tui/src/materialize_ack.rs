//! The ack/reaction side of the save flow: recording what the caller-side
//! `vfs` work concluded, the snapshot-autosave enqueue, the dirty-cache
//! recompute chokepoint, and `on_store_failure`'s whole-store degrade.
//! `save` owns building and submitting the materialize/save operation in
//! the first place; this module owns everything from the recovery store's
//! first reply onward.
//!
//! Every function here is per-document except `on_store_failure`, which
//! stays app-wide: a hard write failure degrades the ONE shared `Store`,
//! never just the document that happened to trigger it — but its own sweep
//! is state-aware per document (`Document::save_phase`), rather than
//! abandoning every in-flight save uniformly.
//!
//! Two siblings carry the halves that would otherwise push this file past
//! the 500-line budget. [`publish`] holds the prepare ack (with its
//! divergence gate), the `vfs` `Cmd` it spawns, and that `Cmd`'s own
//! outcome reaction. [`reactions`] holds what happens once a save attempt
//! actually resolves — the success/failure arms, the continuation
//! chokepoint they funnel through, and the local materialize failure path.

use std::path::Path;

use rune_db::{MatResult, MaterializeOutcome, SyncKind};

use crate::app::App;
use crate::document::{DocumentId, SavePhase};
use crate::guard::{self, GuardKind, GuardPrompt};
use crate::messages;

mod publish;
mod reactions;
pub use publish::MaterializeVfsOutcome;
pub(crate) use publish::{handle_materialize_vfs_done, handle_prepare_ack};
use reactions::fail_materialize_locally;
pub(crate) use reactions::{handle_materialize_ack, handle_save_done, retire_quit_wait};

pub(crate) const DURABILITY_UNCONFIRMED_WARNING: &str =
    "saved \u{2014} durability unconfirmed; prior content kept at the sibling temp";

/// The one refusal text both save-time disk-conflict refusals post — the
/// pre-publish divergence gate and the CAS refusal are the same fact to the
/// user (the file holds changes this buffer does not), and must never
/// describe themselves as two different failures.
pub(crate) const SAVE_REFUSED_DISK_CHANGED: &str =
    "save refused \u{2014} the file changed on disk since it was opened";

/// A save-time refusal IS fresh evidence about the disk, and `merge::begin`'s
/// own fast pre-check reads `last_sync`, so leaving it on a stale `Clean`
/// would refuse `[M]erge`/`[D]iscard` the moment the user picked one. The
/// AUTHORITATIVE classification still happens fresh inside the `MergePrep`
/// either answer starts.
pub(crate) fn seed_refusal_classification(app: &mut App, id: DocumentId, kind: SyncKind) {
    if let Some(doc) = app.doc_mut(id) {
        doc.last_sync = Some(kind);
    }
}

/// The chokepoint both of those refusals raise their answer prompt through.
pub(crate) fn raise_disk_conflict(app: &mut App, id: DocumentId, kind: SyncKind) {
    seed_refusal_classification(app, id, kind);
    let _ = guard::set_guard_or_warn(
        app,
        GuardPrompt {
            doc: id,
            kind: GuardKind::DiskConflict,
        },
        "disk-conflict confirmation dropped \u{2014} a prompt is already showing",
    );
}

/// `Recording`'s reaction to its own `MaterializeRecord` ack: `op_id` is
/// checked against the document's own `record_op` before anything else
/// happens — a document that has moved on to a later attempt since this op
/// was enqueued gets a typed, silent drop rather than [`handle_materialize_
/// ack`]'s full reactions running against the wrong capture.
pub(crate) fn handle_materialize_ack_for_op(
    app: &mut App,
    id: DocumentId,
    op_id: u64,
    mat: MatResult,
) {
    if app.doc(id).and_then(super::document::Document::record_op) != Some(op_id) {
        return;
    }
    handle_materialize_ack(app, id, &mat);
}

/// The disk-sourced facts [`record_outcome`] needs to call `rune-db`'s
/// `materialize_record` — bundled so that function stays under clippy's
/// argument-count lint without losing any of the "caller-captured, never
/// re-derived" facts each field carries.
#[derive(Clone, Copy)]
struct RecordTarget<'a> {
    db_id: i64,
    seq: i64,
    content: &'a str,
    resolved_path: &'a Path,
}

/// Hands `outcome` to `rune-db`'s bookkeeping-only `MaterializeRecord` and
/// advances `id` from `Publishing` to `Recording` — `published` marks
/// whether the disk write ALREADY physically completed (`Committed`/
/// `Raced`); `Document::save_phase`'s own `Recording { published }` is what
/// lets `on_store_failure` resolve a published record's lost ack as a
/// synthetic commit instead of abandoning a write that already succeeded.
fn record_outcome(
    app: &mut App,
    id: DocumentId,
    target: RecordTarget<'_>,
    outcome: MaterializeOutcome,
    published: bool,
) {
    let Some(db) = app.db.as_ref() else {
        // No store left at all — the write may have already committed
        // (`published`); either way there is nothing left to record it
        // against, so finish exactly as a committed/refused ack would.
        if published {
            handle_materialize_ack(app, id, &MatResult::Committed { saved: None });
        } else {
            fail_materialize_locally(app, id, "save failed: recovery store unavailable");
        }
        return;
    };
    match db.store.materialize_record(
        rune_db::DocId(target.db_id),
        target.resolved_path,
        target.seq,
        outcome,
    ) {
        Ok(op_id) => {
            app.db_ops.insert(op_id, crate::db::PendingOp::new(id));
            if let Some(doc) = app.doc_mut(id) {
                doc.begin_recording(op_id, published);
            }
        }
        Err(e) => {
            if published {
                // The write already physically committed but its own
                // observation just failed to record — the disk now holds
                // exactly `content`, so a save that starts before the
                // re-baseline load below lands must be able to recognize
                // that as its own echo rather than manufacture a conflict
                // against it (`FileBinding::pending_rebaseline_hash`'s own
                // doc comment). Stashed on the SHARED per-file entry, not
                // this one document's own binding: any OTHER tab open on
                // the same file needs to recognize the identical echo too.
                if let Some(binding) = app.doc_file_binding_mut(id) {
                    binding.pending_rebaseline_hash =
                        Some(rune_db::hash_bytes(target.content.as_bytes()));
                }
                // The write already physically completed — only the DB
                // bookkeeping is lost. Report the save as successful FIRST
                // (resolving this document's `save` back to `Idle`) so the
                // subsequent whole-store degrade doesn't also flag it as a
                // failed save.
                handle_materialize_ack(app, id, &MatResult::Committed { saved: None });
            }
            on_store_failure(app, &e.to_string());
        }
    }
}

/// The counterpart to [`record_outcome`] for a `Committed`/`Raced` outcome
/// arriving for a document that is no longer `live` — closed mid-flight, or
/// moved on to a later attempt. The write already physically took effect,
/// so its bytes are still recorded through `MaterializeRecord`; the
/// enqueued op's `PendingOp` carries no doc-scoped reaction (`db_dispatch`'s
/// own routing already treats an unmatched op id as a no-op), so its
/// eventual ack — or a lost one — never touches any document again.
fn record_orphan_outcome(
    app: &mut App,
    db_id: i64,
    seq: i64,
    resolved_path: &Path,
    outcome: MaterializeOutcome,
) {
    let Some(db) = app.db.as_ref() else { return };
    let _ = db
        .store
        .materialize_record(rune_db::DocId(db_id), resolved_path, seq, outcome);
}

/// A store enqueue-time error or an async `DbEvent::Err`/`Fatal` landed: the
/// in-memory buffer/journal are NEVER rolled back — only the WHOLE store is
/// marked degraded (sticky; no reopen path) and a persistent banner is
/// raised. Every document's own save attempt is swept by its CURRENT
/// `Document::save_phase` (not a uniform "every in-flight save dies"):
/// `Preparing` and an unpublished `Recording` abandon (nothing irrevocable
/// happened yet); a published `Recording` resolves as a synthetic commit
/// (the write already succeeded — only its own bookkeeping ack was lost);
/// `Publishing` and `Direct` are left completely untouched — a vfs `Cmd` is
/// already outstanding for them, headed to (or already on) disk, and the
/// store's death cannot cancel a write already in flight. This state-aware
/// sweep is what makes a second save attempt for a `Publishing` document
/// structurally impossible: `save_in_flight()` stays `true` the whole time,
/// so `trigger_save` keeps refusing.
pub(crate) fn on_store_failure(app: &mut App, error: &str) {
    if let Some(db) = app.db.as_mut() {
        db.degraded = true;
    }
    app.db_banner = Some(format!("recovery disabled: {error}"));
    messages::error(app, format!("recovery disabled: {error}"));

    let ids: Vec<DocumentId> = app.documents.keys().copied().collect();
    let mut abandoned_any = false;
    let mut resolved_committed = Vec::new();
    for id in &ids {
        // A `Binding` document has no durable row yet — a store this
        // degraded will never deliver the `Load`/`CreateScratch` ack that
        // would have installed one, so whatever it buffered can never
        // replay; dropping it here is honest, not a loss on top of a
        // loss — `App::is_preserved` already reports an unbound document's
        // unsaved bytes as unpreserved.
        if let Some(doc) = app.doc_mut(*id)
            && matches!(doc.replica, crate::document::Replica::Binding { .. })
        {
            doc.replica = crate::document::Replica::Detached;
        }
    }
    for id in ids {
        let Some(doc) = app.doc(id) else { continue };
        match doc.save_phase() {
            SavePhase::Preparing | SavePhase::Recording { published: false } => {
                let pending_version = doc.pending_save_version();
                if let Some(doc) = app.doc_mut(id) {
                    doc.abandon_save();
                }
                recompute_dirty(app, id);
                reactions::resolve_continuations(app, id, pending_version, false);
                abandoned_any = true;
            }
            SavePhase::Recording { published: true } => {
                resolved_committed.push(id);
            }
            SavePhase::Idle | SavePhase::Direct | SavePhase::Publishing => {}
        }
    }
    for id in resolved_committed {
        handle_materialize_ack(app, id, &MatResult::Committed { saved: None });
    }
    if abandoned_any {
        messages::error(app, format!("save failed: {error}"));
    }

    // A `Fatal` kills the writer's whole FIFO — every op it had enqueued,
    // including any quit-save fan-out's, will now never ack. Aborting the
    // intent outright (rather than leaving it to strand forever, waiting on
    // acks that can no longer arrive) is what keeps the NEXT `^C` able to
    // raise a fresh, resolvable Guard instead of silently doing nothing —
    // the failure itself is already surfaced via `db_banner` above, so no
    // separate status is needed here.
    app.quit_intent = None;
}

/// A stale `generation` (a later journal mutation already rescheduled the
/// debounce — `save::schedule_snapshot_debounce`) is ignored. `content` is
/// captured SYNCHRONOUSLY here, in `update` — never re-derived once the
/// enqueued `CreateSnapshot` op actually runs on the writer thread. Never
/// touches the user's file — `create_snapshot` is a pure recovery anchor.
pub(crate) fn handle_snapshot_due(app: &mut App, id: DocumentId, generation: u32) {
    if app.db.as_ref().is_none_or(|db| db.degraded) {
        return;
    }
    let Some(doc) = app.doc(id) else { return };
    let Some(db_id) = doc
        .doc_db()
        .filter(|d| d.snapshot_generation == generation)
        .map(|d| d.db_id)
    else {
        return;
    };
    let content = doc.buffer.content().to_string();
    let Some(db) = app.db.as_ref() else { return };
    let result = db.store.create_snapshot(rune_db::DocId(db_id), &content);
    match result {
        Ok(op_id) => {
            app.db_ops.insert(op_id, crate::db::PendingOp::new(id));
        }
        Err(e) => on_store_failure(app, &e.to_string()),
    }
}

/// `Document::is_dirty` reads only the cache this recomputes. A straight
/// content comparison against `saved_content` — never a version proxy:
/// `Buffer::apply_edits` always returns `version + 1`, and undo/redo build
/// a fresh buffer, so a version comparison alone leaves an edit-then-undo
/// document dirty forever even once the bytes are back to identical.
pub(crate) fn recompute_dirty(app: &mut App, id: DocumentId) {
    let Some(doc) = app.doc(id) else { return };
    let dirty = doc.buffer.content() != &*doc.saved_content;
    let Some(doc) = app.doc_mut(id) else { return };
    doc.is_dirty_cached = dirty;
}

/// Dirty must be re-derived on every TRANSITION (open,
/// switch, evict, close, quit), not merely read from the render-only cache
/// `recompute_dirty`'s other callers (edit/ack sites) already keep current
/// between transitions. Every transition-time dirty check — the close-guard
/// predicate, `workspace::request_close`, and the quit-guard's scan over
/// unpreserved documents — calls this instead of `Document::is_dirty` so a
/// transition's answer is never one edit/ack stale — render is the one place
/// that keeps reading the cache.
pub(crate) fn is_dirty_now(app: &mut App, id: DocumentId) -> bool {
    recompute_dirty(app, id);
    app.doc(id).is_some_and(super::document::Document::is_dirty)
}
