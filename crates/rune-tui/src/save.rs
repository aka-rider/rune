//! The save/ack/dirty flow (plan WP1.S5, extracted out of `app.rs` to keep
//! it under the §1.6 line budget): `trigger_save`'s degraded-store confirm
//! gate, `materialize_now`'s CAS-enqueue, the `Msg::SaveDone`/materialize-ack
//! reactions, the dirty-cache recompute chokepoint (§1.4.8), the snapshot-
//! autosave debounce, and `on_store_failure`'s whole-store degrade. Every
//! function here is per-document except `on_store_failure`, which stays
//! app-wide (plan decision 3/6: a hard write failure degrades the ONE
//! shared `Store`, never just the document that happened to trigger it).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rune_db::MatResult;
use rune_vfs::Vfs;

use crate::app::{App, StatusSource};
use crate::document::DocumentId;
use crate::runtime::{Cmd, CmdKind, Effects, Msg};
use crate::workspace;

/// The degraded-save confirm-gate's arm-to-confirm window — mirrors
/// `app::CONFIRM_TIMEOUT` (plan WP5.S2/S6: "a pending-confirm state like the
/// existing quit-confirm pattern").
const SAVE_CONFIRM_TIMEOUT: Duration = Duration::from_secs(2);

/// The snapshot-autosave debounce window (plan WP5.S6, port of
/// `workspace_timers.go:11`'s 2s debounce).
const SNAPSHOT_DEBOUNCE: Duration = Duration::from_secs(2);

/// `super+s` (WP9, plan Context "Save"; WP5.S6 routes it through
/// `rune-db`'s `materialize` on the writer FIFO when a store is present).
/// Guarded by `id`'s in-flight flag (a second `super+s` before the first
/// save's ack reports back is a no-op) and by `version != saved_version`
/// (nothing to persist otherwise).
///
/// When the store is degraded (open-ladder fallback or a later
/// `on_store_failure`), the FIRST `super+s` only arms a confirm gate
/// tagged with `id` (plan WP1 decision 3, mirrors `app::handle_quit_key`'s
/// `pending_quit` shape) — a document with no durable recovery journal can
/// still be saved, but only once the user has explicitly acknowledged that
/// crash protection is off; a SECOND `super+s` for the SAME document within
/// the window proceeds.
///
/// With no store at all, or with this particular document unbound to one
/// (Assumption A1: a document opened after WP4's Explorer lands with
/// `db: None` until per-doc hydration exists), falls back to the pre-WP5
/// direct `vfs.save_atomic` `Cmd` — Prime Directive: the user must always be
/// able to save (plan decision 5: "losing the DB never damages a user
/// file").
pub(crate) fn trigger_save(app: &mut App, id: DocumentId, effects: &mut Effects) {
    let Some(doc) = app.doc(id) else { return };
    if doc.save_in_flight {
        return;
    }
    let version = doc.buffer.version();
    if version == doc.saved_version {
        return;
    }
    let Some(path) = doc.file_path.clone() else {
        // A pathless draft (including the default untitled document a
        // no-arg launch opens) has nothing to save yet — ⌘S here means
        // "name it", so route it into the same "pathless draft is a
        // CREATE" flow `rename::begin` already implements
        // (`rename.rs:212-218` -> `bind_new`): focus the title field so the
        // user can type a name; Enter from there commits the create, and
        // `Document::bind_path` (routed through by both `bind_to` and
        // `handle_materialize_ack` below) is what actually switches the
        // title off the placeholder once the file exists.
        crate::pane::focus_title(app);
        app.set_status(
            "name this document to save it \u{2014} press Enter when done",
            StatusSource::Other,
        );
        return;
    };

    let has_binding = app.db.is_some() && doc.db.is_some();
    if !has_binding {
        // No store at all, or this document has no binding to it — the
        // pre-WP5 direct-vfs fallback.
        let bytes = doc.buffer.content().as_bytes().to_vec();
        if let Some(doc) = app.doc_mut(id) {
            doc.save_in_flight = true;
        }
        let vfs = Arc::clone(&app.vfs);
        effects.cmds.push(save_cmd(id, vfs, path, bytes, version));
        return;
    }

    let degraded = app.db.as_ref().is_some_and(|db| db.degraded);
    if degraded {
        if app.pending_save_confirm.is_some_and(|(cid, _)| cid == id) {
            app.pending_save_confirm = None;
            materialize_now(app, id, path, version);
        } else {
            let generation = app.next_save_confirm_gen;
            app.next_save_confirm_gen = app.next_save_confirm_gen.wrapping_add(1);
            app.pending_save_confirm = Some((id, generation));
            app.set_status(
                "recovery disabled \u{2014} press \u{2318}S again to save anyway",
                StatusSource::Other,
            );
            effects.cmds.push(save_confirm_timeout_cmd(generation));
        }
        return;
    }

    materialize_now(app, id, path, version);
}

/// Enqueues `content` to `rune-db`'s writer FIFO via `Store::materialize`
/// (plan WP5.S6). Not a `Cmd`: `Store::enqueue` is a plain, non-blocking
/// channel send (never I/O that leaves this thread — the actual disk write
/// happens on the writer thread, whose eventual `DbEvent::Ok{ result:
/// OpOutcome::Materialize(..), .. }` ack arrives as `Msg::Db`, routed back
/// to `id` via `app.db_ops`, handled by `handle_materialize_ack`), so §5.4
/// lets `update` call it directly. `content`/`expect`/`seq`/`bind_new` are
/// all captured HERE, synchronously — never re-derived once the op runs
/// (§1.4.2/§1.4.8).
fn materialize_now(app: &mut App, id: DocumentId, path: PathBuf, version: u64) {
    let Some(doc) = app.doc(id) else { return };
    let Some((db_id, expect_obs, last_known_seq, bind_new)) = doc
        .db
        .as_ref()
        .map(|d| (d.db_id, d.expect_obs, d.last_known_seq, d.bind_new))
    else {
        return;
    };
    let content = doc.buffer.content().to_string();
    let Some(db) = app.db.as_ref() else { return };
    let result = db
        .store
        .materialize(db_id, &path, &content, expect_obs, last_known_seq, bind_new);

    if let Some(doc) = app.doc_mut(id) {
        doc.save_in_flight = true;
        doc.save_pending_version = Some(version);
    }
    match result {
        Ok(op_id) => {
            app.db_ops.insert(op_id, id);
        }
        Err(e) => {
            // A store enqueue-time failure is exactly the same class of
            // event as an async `DbEvent::Err`/`Fatal` (plan decision 3) —
            // degrade the store and raise the sticky banner via the same
            // chokepoint `db::append_edit`/`move_undo_pos` use, not a
            // one-shot `SaveError` status that leaves the store untouched
            // and lets the next save silently retry against an already-
            // wedged writer.
            on_store_failure(app, e.to_string());
        }
    }
}

/// The draft-naming route (`rename::bind_new`): materialize the buffer to
/// `path` with `bind_new=true` — an atomic no-clobber `rename_excl` create
/// whose EEXIST branch refuses and records the winner's bytes
/// (`materialize_create`).
///
/// `trigger_save` cannot be reused here: it reads `doc.file_path`, which is
/// exactly what a draft does not have yet. And the document is deliberately
/// NOT bound to `path` up front — a `rename_excl` that loses the race must
/// leave the draft untitled, or a later ⌘S would overwrite the winner
/// (§0.1 rung 1). `handle_materialize_ack` performs the bind once the
/// write actually commits.
pub(crate) fn bind_new_now(app: &mut App, id: DocumentId, path: PathBuf) {
    let Some(doc) = app.doc(id) else { return };
    if doc.save_in_flight {
        return;
    }
    let version = doc.buffer.version();
    let Some(db_id) = doc.db.as_ref().map(|d| d.db_id) else {
        return;
    };
    let content = doc.buffer.content().to_string();
    let Some(db) = app.db.as_ref() else { return };
    // `expect` is unused on the create path (`materialize_create` never
    // consults it) and `seq` is the live journal position, captured HERE
    // (§1.4.2/§1.4.8).
    let seq = app
        .doc(id)
        .and_then(|d| d.db.as_ref())
        .map(|d| d.last_known_seq)
        .unwrap_or(0);
    let result = db.store.materialize(db_id, &path, &content, 0, seq, true);

    if let Some(doc) = app.doc_mut(id) {
        doc.save_in_flight = true;
        doc.save_pending_version = Some(version);
        // Remembered so the ack can bind it — see `pending_bind_path`.
        doc.pending_bind_path = Some(path);
    }
    match result {
        Ok(op_id) => {
            app.db_ops.insert(op_id, id);
        }
        Err(e) => {
            if let Some(doc) = app.doc_mut(id) {
                doc.save_in_flight = false;
                doc.pending_bind_path = None;
            }
            on_store_failure(app, e.to_string());
        }
    }
}

/// The reaction to a `materialize` ack for `id` (plan WP5.S6): advances
/// `saved_version`/`DocDb::expect_obs`/`bind_new` on a commit, surfaces each
/// `MatResult` outcome as status text, and — either way — clears `id`'s
/// `save_in_flight` and recomputes its dirty cache (trigger (b) of
/// `recompute_dirty`'s doc comment).
pub(crate) fn handle_materialize_ack(app: &mut App, id: DocumentId, mat: MatResult) {
    let Some(doc) = app.doc_mut(id) else { return };
    doc.save_in_flight = false;
    let pending_version = doc.save_pending_version.take();

    if mat.committed {
        // A committed bind-new create is where an untitled draft finally
        // gets its path — only now, after the no-clobber publish actually
        // succeeded (see `bind_new_now`'s docs).
        if let Some(path) = app.doc_mut(id).and_then(|d| d.pending_bind_path.take()) {
            if let Some(doc) = app.doc_mut(id) {
                doc.bind_path(path);
            }
            if app.active == id {
                let stem = app.doc(id).map(crate::title::stem_for).unwrap_or_default();
                app.title.seed(&stem);
            }
        }
        if let Some(saved) = &mat.saved
            && let Some(doc_db) = app.doc_mut(id).and_then(|d| d.db.as_mut())
        {
            doc_db.expect_obs = saved.id;
            doc_db.bind_new = false;
        }
        if let Some(version) = pending_version
            && let Some(doc) = app.doc_mut(id)
            && version > doc.saved_version
        {
            doc.saved_version = version;
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
    } else if mat.missing {
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
    let Some(doc) = app.doc_mut(id) else { return };
    doc.save_in_flight = false;
    let succeeded = result.is_ok();
    match result {
        Ok(()) => {
            if let Some(doc) = app.doc_mut(id)
                && version > doc.saved_version
            {
                doc.saved_version = version;
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
        workspace::close_now(app, id);
    }
}

/// A stale `generation` (a later journal mutation already rescheduled the
/// debounce — `schedule_snapshot_debounce`) is ignored. `content` and the
/// journal position ("current position", plan WP5.S6) are captured
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
            app.db_ops.insert(op_id, id);
        }
        Err(e) => on_store_failure(app, e.to_string()),
    }
}

/// Bumps `id`'s snapshot-autosave generation and (re)arms its 2s debounce
/// deadline on `app`'s one rearmable timer thread (plan WP5.S6, porting the
/// Go reference's own debounce; plan WP16.S5 replaced the previous per-call
/// `Cmd` spawn with `App::snapshot_timer` — see that type's own doc
/// comment) — called once per message batch that mutated the ACTIVE
/// document's journal, from `app::update`'s wrapper. No `Effects` involved
/// any more: arming the timer is a direct, synchronous call, not I/O that
/// needs a spawned thread of its own.
pub(crate) fn schedule_snapshot_debounce(app: &mut App, id: DocumentId) {
    if app.db.is_none() {
        return;
    }
    let Some(doc) = app.doc_mut(id) else { return };
    let Some(doc_db) = doc.db.as_mut() else {
        return;
    };
    doc_db.snapshot_generation = doc_db.snapshot_generation.wrapping_add(1);
    let generation = doc_db.snapshot_generation;
    let deadline = std::time::Instant::now() + SNAPSHOT_DEBOUNCE;
    app.snapshot_timer.arm(id, generation, deadline);
}

/// A store enqueue-time error or an async `DbEvent::Err`/`Fatal` landed
/// (plan decision 3): the in-memory buffer/journal are NEVER rolled back —
/// only the WHOLE store is marked degraded (sticky; no reopen path) and a
/// persistent banner is raised. If ANY document had a save in flight, its
/// guard is released and the failure surfaces as an ordinary save error
/// too, so `trigger_save`'s in-flight guard can never wedge open on a lost
/// ack — app-wide because one shared `Store`'s failure can strand any
/// document currently mid-save on it, not just the one whose op happened
/// to trigger this call.
pub(crate) fn on_store_failure(app: &mut App, error: String) {
    if let Some(db) = app.db.as_mut() {
        db.degraded = true;
    }
    app.db_banner = Some(format!("recovery disabled: {error}"));

    let mut any_in_flight = false;
    for doc in app.documents.values_mut() {
        if doc.save_in_flight {
            doc.save_in_flight = false;
            doc.save_pending_version = None;
            any_in_flight = true;
        }
    }
    if any_in_flight {
        app.set_status(format!("save failed: {error}"), StatusSource::SaveError);
    }
}

/// CONSTITUTION §1.4.8: `Document::is_dirty` reads only the cache this
/// recomputes — called at exactly two trigger points: (a) any journal
/// mutation (`commands::edit::commit_edit_batch`/`undo`/`redo`, immediately
/// after they mutate `doc.journal`) and (b) any `DbEvent` ack that moves the
/// baseline (`handle_materialize_ack`, immediately after `saved_version`
/// itself changes). The comparison is the buffer-version proxy already
/// established pre-WP5 (`saved_version`, now advanced ONLY by a successful
/// materialize ack or the no-store fallback's `SaveDone`) — both trigger
/// points call this immediately after whichever of `saved_version`/
/// `buffer.version()` they just changed, so the cache never observes a
/// stale pairing.
pub(crate) fn recompute_dirty(app: &mut App, id: DocumentId) {
    let Some(doc) = app.doc(id) else { return };
    let dirty = doc.buffer.version() != doc.saved_version;
    let Some(doc) = app.doc_mut(id) else { return };
    doc.is_dirty_cached = dirty;
}

/// The 2s degraded-save confirm-gate timer (plan WP5.S2/S6) — mirrors
/// `app::quit_confirm_timeout_cmd`'s shape exactly. Doc-agnostic (plan WP1
/// decision 3): the doc tag lives in `App::pending_save_confirm`'s `Option`
/// tuple itself, not in this `Msg`.
fn save_confirm_timeout_cmd(generation: u32) -> Cmd {
    Cmd::new(CmdKind::SaveConfirmTimeout, move || {
        std::thread::sleep(SAVE_CONFIRM_TIMEOUT);
        Some(Msg::SaveConfirmTimeout { generation })
    })
}

/// The off-thread save I/O itself: `vfs.save_atomic` (§1.4.1's durable
/// temp-write + atomic publish, or `Mem`'s test double) writes EXACTLY
/// `bytes` — §1.4.5 byte-verbatim, no normalization anywhere on this path.
/// Only reached when `id` has no store binding — see `trigger_save`'s docs.
fn save_cmd(
    id: DocumentId,
    vfs: Arc<dyn Vfs + Send + Sync>,
    path: PathBuf,
    bytes: Vec<u8>,
    version: u64,
) -> Cmd {
    Cmd::new(CmdKind::Save, move || {
        let result = vfs.save_atomic(&path, &bytes).map_err(|e| e.to_string());
        Some(Msg::SaveDone {
            id,
            version,
            result,
        })
    })
}

// The save/ack/dirty-flow unit tests that used to live here moved to
// `tests/save_flow.rs` (plan WP1.S5, same rationale as `app.rs`'s
// extraction: every item they exercise — `App`, `update`, `Msg`,
// `Effects`, `keymap` types, `commands::edit::insert_char` — is already
// public).
