//! The ack/reaction side of the save flow (split out of `save.rs` to keep
//! it under the 500-line budget): reacting to `MaterializePrepare`'s ack
//! by spawning the caller-side `vfs` work, reacting to what that `vfs` work
//! concluded ([`MaterializeVfsOutcome`]), the `Msg::SaveDone`/
//! materialize-ack reactions, the dirty-cache recompute chokepoint,
//! and `on_store_failure`'s whole-store degrade. `save.rs` owns
//! building and submitting the materialize/save operation in the first
//! place (`trigger_save`/`materialize_now`/`bind_new_now`); this module
//! owns everything from the recovery store's first reply onward.
//!
//! Every function here is per-document except `on_store_failure`, which
//! stays app-wide: a hard write failure degrades the ONE shared `Store`,
//! never just the document that happened to trigger it — but its own sweep
//! is now state-aware per document (`Document::save_phase`), rather than
//! abandoning every in-flight save uniformly.
//!
//! [`reactions`] (split out for the 500-line budget) holds what happens
//! once a save/materialize attempt actually resolves — `handle_materialize_
//! ack`/`handle_save_done`'s own success/failure arms, the close-on-save-ack
//! and quit-save-fan-out chokepoints (`close_if_pending`/`quit_if_pending`)
//! they both funnel through, and the local materialize failure path
//! (`fail_materialize_locally`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_db::{MatResult, MaterializeOutcome, StatFacts};
use rune_vfs::Vfs;

use crate::app::App;
use crate::document::{DocumentId, PublishParams, SavePhase, SaveTicket};
use crate::messages;
use crate::runtime::{Cmd, CmdKind, Effects, Msg};
use crate::save;

mod reactions;
use reactions::fail_materialize_locally;
pub(crate) use reactions::{handle_materialize_ack, handle_save_done, retire_quit_wait};

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
    if app.doc(id).and_then(|d| d.record_op()) != Some(op_id) {
        return;
    }
    handle_materialize_ack(app, id, mat);
}

/// `Preparing`'s reaction: `prep` carries the CAS decision data
/// (`rune_db::MaterializePrep`) — no disk-sourced fact of its own — so this
/// only advances `id` from `Preparing` to `Publishing` and spawns the
/// caller-side `vfs` `Cmd` that performs the ENTIRE disk dance. A document
/// that has moved on since this op was enqueued (closed mid-flight, or a
/// stale ack for an attempt this document already abandoned) is a correct,
/// silent no-op — `op_id` is checked against the document's own `prep_op`
/// before anything else happens.
pub(crate) fn handle_prepare_ack(
    app: &mut App,
    id: DocumentId,
    op_id: u64,
    prep: rune_db::MaterializePrep,
    effects: &mut Effects,
) {
    if app.doc(id).and_then(|d| d.prep_op()) != Some(op_id) {
        return;
    }
    // A baseline left unconfirmed by a prior commit whose observation was
    // lost (`FileBinding::pending_rebaseline_hash`'s own doc comment) stands
    // in for `expect_hash` here — the DB's own lookup would otherwise still
    // be answering off the stale row `expect_obs` never advanced past. Once
    // a real observation lands, this returns `None` again and the DB's own
    // hash is used as always. Shared per file, not per document: whichever
    // tab's own lost-bookkeeping commit produced the stash, this document's
    // OWN next save must recognize the same disk bytes as its own echo too.
    let expect_hash = app
        .doc_file_binding(id)
        .and_then(|b| b.pending_rebaseline_hash.clone())
        .unwrap_or(prep.expect_hash);
    let Some(doc) = app.doc_mut(id) else { return };
    let Some((ticket, content, params)) = doc.begin_publishing() else {
        return;
    };
    let vfs = Arc::clone(&app.vfs);
    effects.cmds.push(materialize_vfs_cmd(
        id,
        ticket,
        vfs,
        content,
        params,
        expect_hash,
        prep.bound_path,
    ));
}

/// What the caller-side `vfs` work ([`save::run_materialize_vfs`])
/// concluded — every disk-sourced fact [`handle_materialize_vfs_done`]
/// needs, carried so this module never has to call `vfs` a second time to
/// re-derive any of it.
#[derive(Debug)]
pub enum MaterializeVfsOutcome {
    /// The overwrite target no longer exists (`bind_new=false` only) —
    /// never silently (re)create.
    Missing,
    /// The caller's own target disagrees with the document's bound path —
    /// a caller-bug guard, not an ordinary CAS race. No `vfs` write was
    /// attempted.
    PathDisagreement,
    /// A genuine `vfs` I/O failure. No `rune-db` op is ever enqueued for
    /// this outcome — nothing happened worth recording, and the failure is
    /// specific to this document's save, not the store.
    Error(String),
    /// The live target (or, for `bind_new`, a concurrent creator's file)
    /// didn't match `expect` — no write was attempted; `data`/`stat`
    /// describe whatever is actually on disk now. `confirmed` is the
    /// bracketed read's own verdict — a racer caught mid-external-rewrite
    /// must never masquerade as a stable fact.
    Conflict {
        data: Vec<u8>,
        origin: &'static str,
        stat: StatFacts,
        confirmed: bool,
        resolved_path: PathBuf,
    },
    /// The write committed with no race. `confirmed` is the post-publish
    /// stat's own verdict; `durable: false` means the publish took effect
    /// but its durability confirmation failed — still a success, surfaced
    /// as a warning.
    Committed {
        data: Vec<u8>,
        stat: StatFacts,
        confirmed: bool,
        resolved_path: PathBuf,
        durable: bool,
    },
    /// The write committed AND a racer's displaced bytes were captured in
    /// the same atomic-swap window. `confirmed` describes `stat` only.
    Raced {
        data: Vec<u8>,
        stat: StatFacts,
        confirmed: bool,
        displaced: Vec<u8>,
        displaced_stat: StatFacts,
        resolved_path: PathBuf,
        durable: bool,
    },
}

pub(crate) const DURABILITY_UNCONFIRMED_WARNING: &str =
    "saved \u{2014} durability unconfirmed; prior content kept at the sibling temp";

/// `Publishing`'s own vfs `Cmd` — resolves the destination, CAS-checks it
/// (`!bind_new`), publishes (`exchange`/`rename_excl`), and on a plain
/// overwrite, reads back the displaced bytes to detect a swap-race —
/// entirely through THIS app's own `Vfs` handle, never the writer thread's.
/// `db_id`/`seq`/`content` are captured here at spawn time and echoed back
/// on `Msg::MaterializeVfsDone` — never re-read from the document once this
/// `Cmd` is running, so a `Committed`/`Raced` outcome can still be recorded
/// durably even if the document has since closed (`record_orphan_outcome`).
/// Tagged `CmdKind::Save` (not a new kind) so quit's existing `save_handles`
/// join covers it exactly like the no-store fallback save.
fn materialize_vfs_cmd(
    id: DocumentId,
    ticket: SaveTicket,
    vfs: Arc<dyn Vfs + Send + Sync>,
    content: Arc<str>,
    params: PublishParams,
    expect_hash: String,
    bound_path: Option<String>,
) -> Cmd {
    Cmd::new(CmdKind::Save, move || {
        let outcome = save::run_materialize_vfs(
            vfs.as_ref(),
            &params.path,
            params.bind_new,
            &content,
            &expect_hash,
            bound_path.as_deref(),
            params.mode,
        );
        Some(Msg::MaterializeVfsDone {
            id,
            ticket,
            db_id: params.db_id,
            seq: params.seq,
            content,
            outcome,
        })
    })
}

/// `Publishing`'s reaction: reacts to [`MaterializeVfsOutcome`]. `live` is
/// `true` only when `id` is still `Publishing` on exactly `ticket` — a
/// document that closed, or moved on to a later attempt, mid-flight gets a
/// typed, silent drop for every outcome that never touched disk
/// (`Missing`/`Error`/`Conflict`), but a `Committed`/`Raced` write already
/// took effect regardless of whether anything is still listening, so its
/// bytes are still recorded durably via [`record_orphan_outcome`] — bytes a
/// write displaces are captured before anything discards them, live
/// document or not.
pub(crate) fn handle_materialize_vfs_done(
    app: &mut App,
    id: DocumentId,
    ticket: SaveTicket,
    db_id: i64,
    seq: i64,
    content: Arc<str>,
    outcome: MaterializeVfsOutcome,
) {
    let live = app
        .doc(id)
        .is_some_and(|d| d.save_ticket() == Some(ticket) && d.is_publishing());
    match outcome {
        MaterializeVfsOutcome::Missing => {
            if live {
                handle_materialize_ack(
                    app,
                    id,
                    MatResult {
                        missing: true,
                        ..Default::default()
                    },
                );
            }
        }
        MaterializeVfsOutcome::PathDisagreement => {
            on_store_failure(
                app,
                "materialize refused: caller-supplied path does not match the bound path"
                    .to_string(),
            );
        }
        MaterializeVfsOutcome::Error(e) => {
            if live {
                fail_materialize_locally(app, id, format!("save failed: {e}"));
            }
        }
        MaterializeVfsOutcome::Conflict {
            data,
            origin,
            stat,
            confirmed,
            resolved_path,
        } => {
            if live {
                record_outcome(
                    app,
                    id,
                    RecordTarget {
                        db_id,
                        seq,
                        content: &content,
                        resolved_path: &resolved_path,
                    },
                    MaterializeOutcome::Conflict {
                        data,
                        origin,
                        stat,
                        confirmed,
                    },
                    false,
                );
            }
        }
        MaterializeVfsOutcome::Committed {
            data,
            stat,
            confirmed,
            resolved_path,
            durable,
        } => {
            if !durable {
                messages::warn(app, DURABILITY_UNCONFIRMED_WARNING);
            }
            let outcome = MaterializeOutcome::Committed {
                data,
                stat,
                confirmed,
            };
            if live {
                record_outcome(
                    app,
                    id,
                    RecordTarget {
                        db_id,
                        seq,
                        content: &content,
                        resolved_path: &resolved_path,
                    },
                    outcome,
                    true,
                );
            } else {
                record_orphan_outcome(app, db_id, seq, &resolved_path, outcome);
            }
        }
        MaterializeVfsOutcome::Raced {
            data,
            stat,
            confirmed,
            displaced,
            displaced_stat,
            resolved_path,
            durable,
        } => {
            if !durable {
                messages::warn(app, DURABILITY_UNCONFIRMED_WARNING);
            }
            let outcome = MaterializeOutcome::Raced {
                data,
                stat,
                confirmed,
                displaced,
                displaced_stat,
            };
            if live {
                record_outcome(
                    app,
                    id,
                    RecordTarget {
                        db_id,
                        seq,
                        content: &content,
                        resolved_path: &resolved_path,
                    },
                    outcome,
                    true,
                );
            } else {
                record_orphan_outcome(app, db_id, seq, &resolved_path, outcome);
            }
        }
    }
}

/// The disk-sourced facts [`record_outcome`] needs to call `rune-db`'s
/// `materialize_record` — bundled so that function stays under clippy's
/// argument-count lint without losing any of the "caller-captured, never
/// re-derived" facts each field carries.
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
            handle_materialize_ack(
                app,
                id,
                MatResult {
                    committed: true,
                    ..Default::default()
                },
            );
        } else {
            fail_materialize_locally(app, id, "save failed: recovery store unavailable");
        }
        return;
    };
    match db
        .store
        .materialize_record(target.db_id, target.resolved_path, target.seq, outcome)
    {
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
                handle_materialize_ack(
                    app,
                    id,
                    MatResult {
                        committed: true,
                        ..Default::default()
                    },
                );
            }
            on_store_failure(app, e.to_string());
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
        .materialize_record(db_id, resolved_path, seq, outcome);
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
pub(crate) fn on_store_failure(app: &mut App, error: String) {
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
                if let Some(doc) = app.doc_mut(id) {
                    doc.abandon_save();
                }
                recompute_dirty(app, id);
                abandoned_any = true;
            }
            SavePhase::Recording { published: true } => {
                resolved_committed.push(id);
            }
            SavePhase::Idle | SavePhase::Direct | SavePhase::Publishing => {}
        }
    }
    for id in resolved_committed {
        handle_materialize_ack(
            app,
            id,
            MatResult {
                committed: true,
                ..Default::default()
            },
        );
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
    let result = db.store.create_snapshot(db_id, &content);
    match result {
        Ok(op_id) => {
            app.db_ops.insert(op_id, crate::db::PendingOp::new(id));
        }
        Err(e) => on_store_failure(app, e.to_string()),
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
    app.doc(id).is_some_and(|doc| doc.is_dirty())
}
