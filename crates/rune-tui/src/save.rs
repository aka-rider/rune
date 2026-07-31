//! The enqueue/request side of the save flow (plan WP1.S5, extracted out of
//! `app.rs` to keep it under the §1.6 line budget; the ack/reaction side —
//! everything from the recovery store's first reply onward — split further
//! into [`crate::materialize_ack`]): `trigger_save`'s degraded-store
//! confirm gate, the store-backed materialize dance (WP7: enqueue
//! `MaterializePrepare`, bookkeeping only; once `materialize_ack::
//! handle_prepare_ack` reacts to its ack, it spawns the caller-side `vfs`
//! `Cmd` that runs [`run_materialize_vfs`] — the ENTIRE `vfs` dance,
//! through THIS app's own `Vfs` handle, never the writer thread's, since a
//! dead writer thread must never make saving impossible ([rune-db 1])), and
//! the snapshot-autosave debounce.
//!
//! `App::pending_materialize` carries the caller-captured
//! content/path/CAS facts between these hops (§1.4.2/§1.4.8: captured once,
//! at trigger time, never re-derived).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rune_syntax::DocumentKind;
use rune_vfs::Vfs;

use crate::app::{App, StatusSource};
use crate::document::DocumentId;
use crate::materialize_ack::{self, MaterializeVfsOutcome};
use crate::runtime::{Cmd, CmdKind, Effects, Msg};

/// The degraded-save confirm-gate's arm-to-confirm window — mirrors
/// `app::CONFIRM_TIMEOUT` (plan WP5.S2/S6: "a pending-confirm state like the
/// existing quit-confirm pattern").
const SAVE_CONFIRM_TIMEOUT: Duration = Duration::from_secs(2);

/// The snapshot-autosave debounce window (plan WP5.S6, port of
/// `workspace_timers.go`'s 2s debounce).
const SNAPSHOT_DEBOUNCE: Duration = Duration::from_secs(2);

/// WP7: the content/path/CAS facts `materialize_now`/`bind_new_now` capture
/// at trigger time, held in `App::pending_materialize` between
/// `MaterializePrepare`'s ack (which carries no disk-sourced data of its
/// own) and the caller-side `vfs` `Cmd` it spawns. Never re-derived once
/// captured (§1.4.2/§1.4.8) — the eventual `vfs` work and `MaterializeRecord`
/// enqueue both read only from this struct, never from the document's
/// (possibly further-edited) live buffer.
#[derive(Clone)]
pub(crate) struct PendingMaterialize {
    pub(crate) content: String,
    pub(crate) path: PathBuf,
    pub(crate) bind_new: bool,
    pub(crate) db_id: i64,
    pub(crate) seq: i64,
}

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
    // Plan WP4.S9, the §1.4.1 guard: an image document has a REAL
    // `file_path`, so without this a save would reach `save_cmd` and
    // overwrite it with the buffer's own (always empty) bytes. Placed
    // FIRST, before the in-flight/version checks below — those already
    // return early for an unedited buffer, which would make a guard placed
    // after them dead code.
    if doc.kind == DocumentKind::Image {
        return;
    }
    if doc.save_in_flight {
        return;
    }
    // The mirror of `rename::begin`'s own `save_in_flight` refusal, and
    // required for the same reason from the other side: a save `Cmd`
    // captures the document's path when it is spawned, while the rebind to
    // the renamed path only happens once the rename ack lands. Saving in
    // between republishes the edited content at the OLD path — resurrecting
    // the file the rename is in the middle of moving away from, and leaving
    // the new name holding stale bytes. Refused rather than queued: the ack
    // is one message away, and ⌘S again after it lands does the right thing
    // against the right path.
    if app.rename.in_flight() {
        app.set_status(
            "can't save while a rename is in flight",
            StatusSource::Other,
        );
        return;
    }
    let Some(doc) = app.doc(id) else { return };
    let version = doc.buffer.version();
    if version == doc.saved_version {
        return;
    }
    let Some(path) = doc.file_path.clone() else {
        // A pathless draft (including the default untitled document a
        // no-arg launch opens) has nothing to save yet — ⌘S here means
        // "name it", so route it into the same "pathless draft is a
        // CREATE" flow `rename::begin` already implements
        // (`rename.rs` -> `bind_new`): focus the title field so the
        // user can type a name; Enter from there commits the create, and
        // `Document::bind_path` (routed through by both `bind_to` and
        // `handle_materialize_ack` below) is what actually switches the
        // title off the placeholder once the file exists.
        app.focus_title();
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
            materialize_now(app, id, path, version, effects);
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

    materialize_now(app, id, path, version, effects);
}

/// WP7 step (a): enqueues `MaterializePrepare` — a plain, non-blocking
/// channel send (never I/O that leaves this thread; the writer thread's
/// reply carries no disk-sourced data at all, only DB bookkeeping), so
/// §5.4 lets `update` call it directly. `content`/`path`/`seq`/`bind_new`
/// are all captured HERE, synchronously, into `App::pending_materialize` —
/// never re-derived once the round trip is under way (§1.4.2/§1.4.8).
fn materialize_now(
    app: &mut App,
    id: DocumentId,
    path: PathBuf,
    version: u64,
    effects: &mut Effects,
) {
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
    let result = db.store.materialize_prepare(db_id, expect_obs, bind_new);

    if let Some(doc) = app.doc_mut(id) {
        doc.save_in_flight = true;
        doc.save_pending_version = Some(version);
    }
    match result {
        Ok(op_id) => {
            app.db_ops.insert(op_id, crate::db::PendingOp::new(id));
            app.pending_materialize.insert(
                id,
                PendingMaterialize {
                    content,
                    path,
                    bind_new,
                    db_id,
                    seq: last_known_seq,
                },
            );
        }
        Err(e) => {
            // WP7: nothing has touched disk yet — the store couldn't even
            // perform the bookkeeping-only prepare step. Degrade the store
            // (same signal `on_store_failure` always raised on an
            // enqueue-time error) AND fall back to the uncoordinated
            // direct-vfs write, exactly like a document with no store
            // binding at all: "press ⌘S again to save anyway" must
            // actually save ([rune-db 1]). `on_store_failure`'s in-flight
            // sweep clears `save_in_flight` for every document (it has no
            // way to know this ONE document's save is about to continue
            // via the fallback `Cmd`) — re-arm it right after, or a second
            // ⌘S could race the fallback write still in progress.
            materialize_ack::on_store_failure(app, e.to_string());
            if let Some(doc) = app.doc_mut(id) {
                doc.save_in_flight = true;
            }
            let bytes = content.into_bytes();
            let vfs = Arc::clone(&app.vfs);
            effects.cmds.push(save_cmd(id, vfs, path, bytes, version));
        }
    }
}

/// The draft-naming route (`rename::bind_new`): materialize the buffer to
/// `path` with `bind_new=true` — an atomic no-clobber `rename_excl` create
/// whose EEXIST branch refuses and records the winner's bytes.
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
    // `expect`/CAS never applies on the create path — `prepare_materialize`
    // returns an empty `MaterializePrep` for `bind_new` and the caller-side
    // `vfs` work skips the read/hash-compare accordingly.
    let seq = app
        .doc(id)
        .and_then(|d| d.db.as_ref())
        .map(|d| d.last_known_seq)
        .unwrap_or(0);
    let result = db.store.materialize_prepare(db_id, 0, true);

    if let Some(doc) = app.doc_mut(id) {
        doc.save_in_flight = true;
        doc.save_pending_version = Some(version);
        // Remembered so the ack can bind it — see `pending_bind_path`.
        doc.pending_bind_path = Some(path.clone());
    }
    match result {
        Ok(op_id) => {
            app.db_ops.insert(op_id, crate::db::PendingOp::new(id));
            app.pending_materialize.insert(
                id,
                PendingMaterialize {
                    content,
                    path,
                    bind_new: true,
                    db_id,
                    seq,
                },
            );
        }
        Err(e) => {
            // Unlike `materialize_now`'s overwrite path, there is no
            // equivalent-safety direct-vfs fallback for a bind-new create:
            // a plain `vfs.save_atomic` has no no-clobber guarantee, and
            // `handle_save_done`'s success path never binds `file_path`
            // (only `handle_materialize_ack`'s `pending_bind_path` dance
            // does) — reusing it here would silently create the file
            // without ever giving the draft its name. The buffer itself is
            // never at risk (still safely in memory, `saved_version`
            // untouched), just this ONE draft-naming attempt; the user can
            // retry once a fresh store is available. Tracked as a narrower,
            // pre-existing gap distinct from [rune-db 1] (which is about
            // an ALREADY-bound document's overwrite, not draft creation) —
            // see `TODO-wp7-bind-new-dead-writer.md`.
            if let Some(doc) = app.doc_mut(id) {
                doc.save_in_flight = false;
                doc.save_pending_version = None;
                doc.pending_bind_path = None;
            }
            materialize_ack::on_store_failure(app, e.to_string());
        }
    }
}

/// The `vfs` dance itself, factored out of `materialize_vfs_cmd` so it is
/// plain, synchronous, testable logic. Mirrors the steps the pre-WP7
/// `rune-db::materialize`/`materialize_overwrite`/`materialize_create` used
/// to run on the writer thread, verbatim in shape, just against the
/// CALLER's own `vfs` instead.
pub(crate) fn run_materialize_vfs(
    vfs: &dyn Vfs,
    path: &Path,
    bind_new: bool,
    content: &str,
    expect_hash: &str,
    bound_path: Option<&str>,
) -> MaterializeVfsOutcome {
    let data = content.as_bytes();

    if bind_new {
        let resolved = match vfs.resolve(path) {
            Ok(r) => r,
            Err(e) => return MaterializeVfsOutcome::Error(e.to_string()),
        };
        if let Some(dir) = resolved.parent()
            && !dir.as_os_str().is_empty()
            && let Err(e) = vfs.mkdir_all(dir)
        {
            return MaterializeVfsOutcome::Error(e.to_string());
        }
        let temp = match vfs.write_durable(&resolved, data) {
            Ok(t) => t,
            Err(e) => return MaterializeVfsOutcome::Error(e.to_string()),
        };
        return match vfs.rename_excl(&temp, &resolved) {
            Ok(()) => {
                let stat = rune_db::stat_identity(vfs, &resolved);
                MaterializeVfsOutcome::Committed {
                    data: data.to_vec(),
                    stat,
                    resolved_path: resolved,
                }
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                // A concurrent creator won the race — our own temp is
                // genuinely unneeded (the winner's bytes are what get
                // recorded), safe to discard.
                let _ = vfs.remove(&temp);
                match vfs.read(&resolved) {
                    Ok(live) => {
                        let stat = rune_db::stat_identity(vfs, &resolved);
                        MaterializeVfsOutcome::Conflict {
                            data: live,
                            origin: "probe",
                            stat,
                            resolved_path: resolved,
                        }
                    }
                    Err(e) => MaterializeVfsOutcome::Error(e.to_string()),
                }
            }
            // Deliberately NOT removed on a genuine I/O failure: the temp
            // is the only place the user's just-written bytes still
            // physically exist.
            Err(e) => MaterializeVfsOutcome::Error(e.to_string()),
        };
    }

    let resolved = match vfs.resolve(path) {
        Ok(r) => r,
        Err(e) => return MaterializeVfsOutcome::Error(e.to_string()),
    };
    if let Some(bound) = bound_path {
        match vfs.resolve(Path::new(bound)) {
            Ok(db_resolved) if db_resolved != resolved => {
                return MaterializeVfsOutcome::PathDisagreement;
            }
            Ok(_) => {}
            Err(e) => return MaterializeVfsOutcome::Error(e.to_string()),
        }
    }

    // Step 1: unconditional read+hash of the live target.
    let live_data = match vfs.read(&resolved) {
        Ok(d) => d,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            // §1.4.4: an ordinary overwrite-intent save must never silently
            // (re)create a file the caller didn't explicitly ask to.
            return MaterializeVfsOutcome::Missing;
        }
        Err(e) => return MaterializeVfsOutcome::Error(e.to_string()),
    };

    // Step 2: live hash != expect -> refuse, no write.
    if rune_db::hash_bytes(&live_data) != expect_hash {
        let stat = rune_db::stat_identity(vfs, &resolved);
        return MaterializeVfsOutcome::Conflict {
            data: live_data,
            origin: "probe",
            stat,
            resolved_path: resolved,
        };
    }

    let temp = match vfs.write_durable(&resolved, data) {
        Ok(t) => t,
        Err(e) => return MaterializeVfsOutcome::Error(e.to_string()),
    };

    match vfs.exchange(&temp, &resolved) {
        Ok(()) => {}
        Err(e) if rune_vfs::published_not_durable(&e) => {
            // WP1: the swap already physically took effect — only the
            // durability CONFIRMATION (parent fsync) failed. `resolved`
            // already holds our new bytes, so this is the same physical
            // state as `Ok(())`; keep going rather than reporting a save
            // that, on disk, actually succeeded.
        }
        Err(e) => {
            // Deliberately NOT removed: the temp is the only place the
            // user's just-written bytes still physically exist.
            return MaterializeVfsOutcome::Error(e.to_string());
        }
    }

    // Step 4: temp now holds what USED TO be at `resolved` (the displaced
    // bytes, never unlinked by the swap) — read+hash it.
    let displaced = match vfs.read(&temp) {
        Ok(d) => d,
        Err(e) => return MaterializeVfsOutcome::Error(e.to_string()),
    };
    let stat = rune_db::stat_identity(vfs, &resolved);
    if rune_db::hash_bytes(&displaced) != expect_hash {
        // F5 swap-race: a writer raced us inside the atomic-swap window.
        let displaced_stat = rune_db::stat_identity(vfs, &temp);
        let _ = vfs.remove(&temp);
        return MaterializeVfsOutcome::Raced {
            data: data.to_vec(),
            stat,
            displaced,
            displaced_stat,
            resolved_path: resolved,
        };
    }
    // Hash matched: `temp` has already been read and verified above, its
    // job done — a failed cleanup here just leaks a scratch file.
    let _ = vfs.remove(&temp);
    MaterializeVfsOutcome::Committed {
        data: data.to_vec(),
        stat,
        resolved_path: resolved,
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
/// Reached when `id` has no store binding (see `trigger_save`'s docs), or
/// as WP7's fallback when a store binding exists but its `MaterializePrepare`
/// enqueue itself failed (the store couldn't even do the bookkeeping-only
/// first step) — either way, the Prime Directive holds: the user can
/// always save.
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
