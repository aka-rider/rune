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
//! stays app-wide (plan decision 3/6: a hard write failure degrades the ONE
//! shared `Store`, never just the document that happened to trigger it).
//!
//! [`reactions`] (split out for the 500-line budget, plan WP2) holds what
//! happens once a save/materialize attempt actually resolves — `handle_
//! materialize_ack`/`handle_save_done`'s own success/failure arms, the
//! close-on-save-ack and quit-save-fan-out chokepoints (`close_if_pending`/
//! `quit_if_pending`) they both funnel through, and the local materialize
//! failure path (`fail_materialize_locally`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_db::{MatResult, MaterializeOutcome, StatFacts};
use rune_vfs::Vfs;

use crate::app::App;
use crate::document::DocumentId;
use crate::messages;
use crate::runtime::{Cmd, CmdKind, Effects, Msg};
use crate::save::{self, PendingMaterialize};

mod reactions;
use reactions::fail_materialize_locally;
pub(crate) use reactions::{handle_materialize_ack, handle_save_done, retire_quit_wait};

/// WP7 step (a)'s reaction: `prep` carries the CAS decision data
/// (`rune_db::MaterializePrep`) — no disk-sourced fact of its own — so this
/// only retrieves `id`'s captured [`PendingMaterialize`] and spawns the
/// caller-side `vfs` `Cmd` that performs the ENTIRE disk dance.  A missing
/// `pending_materialize` entry (the document closed mid-flight, or a stale
/// ack) is a correct, silent no-op.
pub(crate) fn handle_prepare_ack(
    app: &mut App,
    id: DocumentId,
    prep: rune_db::MaterializePrep,
    effects: &mut Effects,
) {
    let Some(pending) = app.pending_materialize.get(&id).cloned() else {
        return;
    };
    let vfs = Arc::clone(&app.vfs);
    effects.cmds.push(materialize_vfs_cmd(
        id,
        vfs,
        pending.path,
        pending.bind_new,
        pending.content,
        prep.expect_hash,
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
    /// a caller-bug guard ([rune-db 5]), not an ordinary CAS race. No `vfs`
    /// write was attempted.
    PathDisagreement,
    /// A genuine `vfs` I/O failure. No `rune-db` op is ever enqueued for
    /// this outcome — nothing happened worth recording, and the failure is
    /// specific to this document's save, not the store.
    Error(String),
    /// The live target (or, for `bind_new`, a concurrent creator's file)
    /// didn't match `expect` — no write was attempted; `data`/`stat`
    /// describe whatever is actually on disk now.
    Conflict {
        data: Vec<u8>,
        origin: &'static str,
        stat: StatFacts,
        resolved_path: PathBuf,
    },
    /// The write committed with no race.
    Committed {
        data: Vec<u8>,
        stat: StatFacts,
        resolved_path: PathBuf,
    },
    /// The write committed AND a racer's displaced bytes were captured in
    /// the same atomic-swap window (F5).
    Raced {
        data: Vec<u8>,
        stat: StatFacts,
        displaced: Vec<u8>,
        displaced_stat: StatFacts,
        resolved_path: PathBuf,
    },
}

/// WP7 step (b): the caller-side `vfs` `Cmd` — resolves the destination,
/// CAS-checks it (`!bind_new`), publishes (`exchange`/`rename_excl`), and
/// on a plain overwrite, reads back the displaced bytes to detect a
/// swap-race — entirely through THIS app's own `Vfs` handle, never the
/// writer thread's. Tagged `CmdKind::Save` (not a new kind) so quit's
/// existing `save_handles` join covers it exactly like the no-store
/// fallback save ([rune-tui A 5]).
fn materialize_vfs_cmd(
    id: DocumentId,
    vfs: Arc<dyn Vfs + Send + Sync>,
    path: PathBuf,
    bind_new: bool,
    content: String,
    expect_hash: String,
    bound_path: Option<String>,
) -> Cmd {
    Cmd::new(CmdKind::Save, move || {
        let outcome = save::run_materialize_vfs(
            vfs.as_ref(),
            &path,
            bind_new,
            &content,
            &expect_hash,
            bound_path.as_deref(),
        );
        Some(Msg::MaterializeVfsDone { id, outcome })
    })
}

/// WP7 step (b)'s reaction: reacts to [`MaterializeVfsOutcome`]. `Missing`
/// finishes locally (no DB round-trip, matching the pre-WP7 behavior of
/// never touching the DB for a missing target). `PathDisagreement` is a
/// caller-bug signal — degrades the whole store, same as the pre-WP7
/// `Error::Invalid` path did via `DbEvent::Err`. A plain `Error` fails only
/// THIS document's save (a disk I/O hiccup is not a `rune-db` failure now
/// that the write no longer runs through it). `Conflict`/`Committed`/
/// `Raced` enqueue `MaterializeRecord` (WP7 step c).
pub(crate) fn handle_materialize_vfs_done(
    app: &mut App,
    id: DocumentId,
    outcome: MaterializeVfsOutcome,
) {
    let Some(pending) = app.pending_materialize.remove(&id) else {
        return;
    };
    match outcome {
        MaterializeVfsOutcome::Missing => {
            handle_materialize_ack(
                app,
                id,
                MatResult {
                    missing: true,
                    ..Default::default()
                },
            );
        }
        MaterializeVfsOutcome::PathDisagreement => {
            on_store_failure(
                app,
                "materialize refused: caller-supplied path does not match the bound path"
                    .to_string(),
            );
        }
        MaterializeVfsOutcome::Error(e) => {
            fail_materialize_locally(app, id, format!("save failed: {e}"));
        }
        MaterializeVfsOutcome::Conflict {
            data,
            origin,
            stat,
            resolved_path,
        } => {
            record_outcome(
                app,
                id,
                &pending,
                &resolved_path,
                MaterializeOutcome::Conflict { data, origin, stat },
                false,
            );
        }
        MaterializeVfsOutcome::Committed {
            data,
            stat,
            resolved_path,
        } => {
            record_outcome(
                app,
                id,
                &pending,
                &resolved_path,
                MaterializeOutcome::Committed { data, stat },
                true,
            );
        }
        MaterializeVfsOutcome::Raced {
            data,
            stat,
            displaced,
            displaced_stat,
            resolved_path,
        } => {
            record_outcome(
                app,
                id,
                &pending,
                &resolved_path,
                MaterializeOutcome::Raced {
                    data,
                    stat,
                    displaced,
                    displaced_stat,
                },
                true,
            );
        }
    }
}

/// WP7 step (c)'s enqueue: hands `outcome` to `rune-db`'s bookkeeping-only
/// `MaterializeRecord`. `published` marks whether the disk write ALREADY
/// physically completed (`Committed`/`Raced`) — when it did, the op id is
/// ALSO recorded in `App::published_ops`, so a dead writer failing this
/// exact op still reports the save as successful (only the store degrades,
/// [rune-db 1]'s "the vfs publish still completes" guarantee).
fn record_outcome(
    app: &mut App,
    id: DocumentId,
    pending: &PendingMaterialize,
    resolved_path: &Path,
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
        .materialize_record(pending.db_id, resolved_path, pending.seq, outcome)
    {
        Ok(op_id) => {
            app.db_ops.insert(op_id, crate::db::PendingOp::new(id));
            if published {
                app.published_ops.insert(op_id, id);
            }
        }
        Err(e) => {
            if published {
                // WP7: the write already physically completed — only the
                // DB bookkeeping is lost. Report the save as successful
                // FIRST (clearing this document's in-flight/pending state)
                // so the subsequent whole-store degrade doesn't also flag
                // it as a failed save.
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

/// A store enqueue-time error or an async `DbEvent::Err`/`Fatal` landed
/// (plan decision 3): the in-memory buffer/journal are NEVER rolled back —
/// only the WHOLE store is marked degraded (sticky; no reopen path) and a
/// persistent banner is raised. If ANY document had a save in flight, its
/// guard is released and the failure surfaces as an ordinary save error
/// too, so `trigger_save`'s in-flight guard can never wedge open on a lost
/// ack — app-wide because one shared `Store`'s failure can strand any
/// document currently mid-save on it, not just the one whose op happened
/// to trigger this call. A document whose write ALREADY physically
/// completed (WP7's `published_ops`) must have been cleared of
/// `save_in_flight` by its own synthetic `handle_materialize_ack` call
/// BEFORE this runs — see `record_outcome`/`dispatch::handle_db_event` — so
/// this loop never re-reports that document's already-successful save as
/// failed.
pub(crate) fn on_store_failure(app: &mut App, error: String) {
    if let Some(db) = app.db.as_mut() {
        db.degraded = true;
    }
    app.db_banner = Some(format!("recovery disabled: {error}"));
    messages::error(app, format!("recovery disabled: {error}"));

    // Collected first: `abandon_save` needs `&mut Document`, and every one
    // ALSO needs its dirty cache re-settled (plan WP1 — a stranded capture
    // must never be left promotable, and the cache must never be left
    // stale after the state it was derived from just changed).
    let stranded: Vec<DocumentId> = app
        .documents
        .iter()
        .filter(|(_, doc)| doc.save_in_flight)
        .map(|(&id, _)| id)
        .collect();
    for &id in &stranded {
        if let Some(doc) = app.doc_mut(id) {
            doc.abandon_save();
        }
        recompute_dirty(app, id);
    }
    if !stranded.is_empty() {
        messages::error(app, format!("save failed: {error}"));
    }

    // A `Fatal` kills the writer's whole FIFO — every op it had enqueued,
    // including any quit-save fan-out's, will now never ack (plan WP2).
    // Aborting the intent outright (rather than leaving it to strand
    // forever, waiting on acks that can no longer arrive) is what keeps the
    // NEXT `^C` able to raise a fresh, resolvable Guard instead of silently
    // doing nothing — the failure itself is already surfaced via `db_banner`
    // above, so no separate status is needed here.
    app.quit_intent = None;
}

/// A stale `generation` (a later journal mutation already rescheduled the
/// debounce — `save::schedule_snapshot_debounce`) is ignored. `content` is
/// captured SYNCHRONOUSLY here, in `update` — never re-derived once the
/// enqueued `CreateSnapshot` op actually runs on the writer thread (the
/// "caller-captured, never re-derived" discipline, same as `materialize`).
/// The durable journal position this content anchors against is NOT
/// captured here — the writer thread resolves it fresh, at op-execution
/// time, from its own already-committed state (`rune_db::OpKind::
/// CreateSnapshot`'s own doc comment), which this app-side call has no way
/// to know exactly while other ops for this doc may still be in flight.
/// Never touches the user's file — `create_snapshot` is a pure recovery
/// anchor (`rune-db::snapshot`'s doc comment).
pub(crate) fn handle_snapshot_due(app: &mut App, id: DocumentId, generation: u32) {
    if app.db.as_ref().is_none_or(|db| db.degraded) {
        return;
    }
    let Some(doc) = app.doc(id) else { return };
    let Some(db_id) = doc
        .db
        .as_ref()
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

/// `Document::is_dirty` reads only the cache this
/// recomputes. A straight content comparison against `saved_content` (plan
/// WP1) — never the old `buffer.version() != saved_version` proxy:
/// `Buffer::apply_edits` always returns `version + 1`, and undo/redo build
/// a fresh buffer, so a version comparison alone leaves an edit-then-undo
/// document dirty forever even once the bytes are back to identical.
/// `saved_content` is a plain `String` compare — a length check plus
/// `memcmp`, microsecond-scale even against a multi-thousand-line document,
/// so this stays cheap enough to call from every edit/ack site AND from
/// [`is_dirty_now`]'s transition re-derive.
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
