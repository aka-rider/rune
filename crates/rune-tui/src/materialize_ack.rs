//! The ack/reaction side of the save flow (split out of `save.rs` to keep
//! it under the §1.6 line budget): reacting to `MaterializePrepare`'s ack
//! by spawning the caller-side `vfs` work, reacting to what that `vfs` work
//! concluded ([`MaterializeVfsOutcome`]), the `Msg::SaveDone`/
//! materialize-ack reactions, the dirty-cache recompute chokepoint
//! (§1.4.8), and `on_store_failure`'s whole-store degrade. `save.rs` owns
//! building and submitting the materialize/save operation in the first
//! place (`trigger_save`/`materialize_now`/`bind_new_now`); this module
//! owns everything from the recovery store's first reply onward.
//!
//! Every function here is per-document except `on_store_failure`, which
//! stays app-wide (plan decision 3/6: a hard write failure degrades the ONE
//! shared `Store`, never just the document that happened to trigger it).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_db::{MatResult, MaterializeOutcome, StatFacts};
use rune_vfs::Vfs;

use crate::app::{App, StatusSource};
use crate::document::DocumentId;
use crate::runtime::{Cmd, CmdKind, Effects, Msg};
use crate::save::{self, PendingMaterialize};
use crate::workspace;

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
    /// §1.4.4: never silently (re)create.
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

/// A local (non-`rune-db`) materialize failure: a genuine `vfs` I/O error
/// on the caller-side write, or a store having vanished entirely
/// mid-flight. Fails only `id`'s save — never the whole store — since the
/// write's own failure carries no `rune-db` signal at all.
fn fail_materialize_locally(app: &mut App, id: DocumentId, message: impl Into<String>) {
    if let Some(doc) = app.doc_mut(id) {
        doc.abandon_save();
        doc.pending_bind_path = None;
    }
    app.set_status(message.into(), StatusSource::SaveError);
    recompute_dirty(app, id);
}

/// The reaction to a `materialize` ack for `id` (plan WP5.S6, re-shaped by
/// WP7's `MaterializeRecord`): advances `saved_version`/`DocDb::expect_obs`/
/// `bind_new` on a commit, surfaces each `MatResult` outcome as status text,
/// and — either way — clears `id`'s `save_in_flight` and recomputes its
/// dirty cache (trigger (b) of `recompute_dirty`'s doc comment). Also
/// called synthetically (WP7, a `committed: true`, otherwise-default
/// `MatResult`) when the disk write physically succeeded but the DB-side
/// bookkeeping that would have supplied `saved`/`raced` was lost to a dead
/// writer — see `record_outcome`'s doc comment.
pub(crate) fn handle_materialize_ack(app: &mut App, id: DocumentId, mat: MatResult) {
    let Some(doc) = app.doc(id) else { return };
    // `MatResult` carries no version of its own — this peek is how the
    // chokepoint below correlates the ack against the SAME bytes
    // `begin_save` captured, never a later unrelated capture (plan WP1).
    let pending_version = doc.pending_save_version();

    if mat.committed {
        // A committed bind-new create is where an untitled draft finally
        // gets its path — only now, after the no-clobber publish actually
        // succeeded (see `bind_new_now`'s docs).
        if let Some(path) = app.doc_mut(id).and_then(|d| d.pending_bind_path.take()) {
            if let Some(doc) = app.doc_mut(id) {
                doc.bind_path(path);
            }
            if app.active == id {
                let name = app.doc(id).map(crate::title::name_for).unwrap_or_default();
                app.title.seed(&name);
            }
        }
        if let Some(saved) = &mat.saved
            && let Some(doc_db) = app.doc_mut(id).and_then(|d| d.db.as_mut())
        {
            doc_db.expect_obs = saved.id;
            doc_db.bind_new = false;
        }
        if let Some(doc) = app.doc_mut(id) {
            match pending_version {
                Some(version) => {
                    doc.finish_save_ok(version);
                }
                None => doc.abandon_save(),
            }
        }
        if app.status_source == StatusSource::SaveError {
            app.status_message = None;
        }
        if mat.raced {
            app.set_status(
                "saved \u{2014} a concurrent external change was overwritten; its bytes were preserved",
                StatusSource::Other,
            );
        }
    } else {
        if let Some(doc) = app.doc_mut(id) {
            doc.abandon_save();
        }
        if mat.missing {
            app.set_status(
                "save failed: file no longer exists",
                StatusSource::SaveError,
            );
        } else {
            app.set_status(
                "save refused \u{2014} the file changed on disk since it was opened",
                StatusSource::SaveError,
            );
        }
    }
    recompute_dirty(app, id);
    close_if_pending(app, id, mat.committed);
}

/// The reaction to `Msg::SaveDone` — the no-store fallback save path's own
/// completion (plan decision 5), or a leftover reply for a document whose
/// store binding vanished mid-flight. Same provenance-aware clear as
/// `handle_materialize_ack` (review finding F2).
pub(crate) fn handle_save_done(
    app: &mut App,
    id: DocumentId,
    version: u64,
    result: Result<(), String>,
) {
    let succeeded = result.is_ok();
    match result {
        Ok(()) => {
            if let Some(doc) = app.doc_mut(id) {
                doc.finish_save_ok(version);
            }
            // Provenance-aware clear (review finding F2): only a message
            // THIS save path set (a prior failed/un-attempted save) is
            // dismissed here. An unrelated message — a pbpaste failure, an
            // edit/undo/redo failure — survives a successful save exactly
            // as it already survives a successful edit.
            if app.status_source == StatusSource::SaveError {
                app.status_message = None;
            }
        }
        Err(e) => {
            if let Some(doc) = app.doc_mut(id) {
                doc.abandon_save();
            }
            app.set_status(format!("save failed: {e}"), StatusSource::SaveError);
        }
    }
    recompute_dirty(app, id);
    close_if_pending(app, id, succeeded);
}

/// The close-on-save-ack chokepoint (plan WP5.S3): both save completion
/// paths (`handle_materialize_ack`'s store-backed flow and this module's
/// own no-store `handle_save_done` fallback — Assumption A1 documents with
/// `db: None` take THIS path, never the other) funnel through here. Only
/// closes when `id` is STILL the document `pending_close_on_save` names —
/// a later unrelated `^w`/Guard interaction on a DIFFERENT document
/// overwrites that single global slot, which correctly abandons (never
/// mis-fires) this document's stale close intent — and only when the save
/// itself actually succeeded; a failed save leaves the document open with
/// its usual error surfaced instead of losing the user's only path back to
/// it.
fn close_if_pending(app: &mut App, id: DocumentId, succeeded: bool) {
    if app.pending_close_on_save != Some(id) {
        return;
    }
    app.pending_close_on_save = None;
    if succeeded {
        // A scratch sink, discarded — see `workspace::close::close_now`'s
        // own doc comment: this call chain never touches an image document.
        let mut effects = Effects::default();
        let _ = workspace::close_now(app, id, &mut effects);
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
        app.set_status(format!("save failed: {error}"), StatusSource::SaveError);
    }
}

/// A stale `generation` (a later journal mutation already rescheduled the
/// debounce — `save::schedule_snapshot_debounce`) is ignored. `content` and
/// the journal position ("current position", plan WP5.S6) are captured
/// SYNCHRONOUSLY here, in `update` — never re-derived once the enqueued
/// `CreateSnapshot` op actually runs on the writer thread (§1.4.2/§1.4.8's
/// "caller-captured, never re-derived" discipline, same as `materialize`).
/// Never touches the user's file — `create_snapshot` is a pure recovery
/// anchor (`rune-db::snapshot`'s doc comment).
pub(crate) fn handle_snapshot_due(app: &mut App, id: DocumentId, generation: u32) {
    if app.db.as_ref().is_none_or(|db| db.degraded) {
        return;
    }
    let Some(doc) = app.doc(id) else { return };
    let Some((db_id, last_known_seq)) = doc
        .db
        .as_ref()
        .filter(|d| d.snapshot_generation == generation)
        .map(|d| (d.db_id, d.last_known_seq))
    else {
        return;
    };
    let content = doc.buffer.content().to_string();
    let Some(db) = app.db.as_ref() else { return };
    let result = db.store.create_snapshot(db_id, &content, last_known_seq);
    match result {
        Ok(op_id) => {
            app.db_ops.insert(op_id, crate::db::PendingOp::new(id));
        }
        Err(e) => on_store_failure(app, e.to_string()),
    }
}

/// CONSTITUTION §1.4.8: `Document::is_dirty` reads only the cache this
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

/// CONSTITUTION §1.4.8: dirty must be re-derived on every TRANSITION (open,
/// switch, evict, close, quit), not merely read from the render-only cache
/// `recompute_dirty`'s other callers (edit/ack sites) already keep current
/// between transitions. The close-guard predicate, `workspace::request_close`,
/// and `pane::first_unpreserved_dirty_doc`'s quit-guard scan all call this
/// instead of `Document::is_dirty` so a transition's answer is never one
/// edit/ack stale — render is the one place that keeps reading the cache.
pub(crate) fn is_dirty_now(app: &mut App, id: DocumentId) -> bool {
    recompute_dirty(app, id);
    app.doc(id).is_some_and(|doc| doc.is_dirty())
}
