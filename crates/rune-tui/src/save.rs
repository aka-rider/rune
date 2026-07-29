//! The save/ack/dirty flow (plan WP1.S5, extracted out of `app.rs` to keep
//! it under the §1.6 line budget): `trigger_save`'s degraded-store confirm
//! gate, the store-backed materialize dance (WP7: `MaterializePrepare`'s
//! ack -> the caller-side `vfs` `Cmd` -> `MaterializeRecord`'s ack),
//! the `Msg::SaveDone`/materialize-ack reactions, the dirty-cache recompute
//! chokepoint (§1.4.8), the snapshot-autosave debounce, and
//! `on_store_failure`'s whole-store degrade. Every function here is
//! per-document except `on_store_failure`, which stays app-wide (plan
//! decision 3/6: a hard write failure degrades the ONE shared `Store`,
//! never just the document that happened to trigger it).
//!
//! # WP7: the disk publish leaves the writer thread
//!
//! A store-backed save used to be a single `rune-db` op — enqueued here,
//! executed entirely on the writer thread (`vfs` calls and all), acked back
//! as one `MatResult`. That made `rune-db` the file write's caller, not its
//! sibling: a dead writer thread made saving impossible ([rune-db 1]).
//! `materialize_now` now drives three hops instead of one:
//!
//! 1. Enqueue `MaterializePrepare` (bookkeeping only, no `vfs` call) —
//!    `handle_prepare_ack` reacts to its ack.
//! 2. `handle_prepare_ack` spawns [`materialize_vfs_cmd`], which performs
//!    the ENTIRE `vfs` dance (resolve/read/hash-compare/`write_durable`/
//!    `exchange` or `rename_excl`/read-displaced) on its own thread, through
//!    THIS app's own `Vfs` handle — never the writer thread's.
//! 3. `handle_materialize_vfs_done` reacts to that `Cmd`'s result: a
//!    `Missing`/`PathDisagreement`/`Error` outcome never touches `rune-db`
//!    again; a `Conflict`/`Committed`/`Raced` outcome enqueues
//!    `MaterializeRecord` so the bookkeeping (blob/observation/rebind) is
//!    recorded, tagging the op in `App::published_ops` whenever the write
//!    itself already physically committed — `dispatch::handle_db_event`
//!    consults that map so a writer dying on THIS op still reports the
//!    save as successful (only the store degrades).
//!
//! `App::pending_materialize` carries the caller-captured
//! content/path/CAS facts between these hops (§1.4.2/§1.4.8: captured once,
//! at trigger time, never re-derived).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rune_db::{MatResult, MaterializeOutcome, StatFacts};
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

/// WP7: the content/path/CAS facts `materialize_now`/`bind_new_now` capture
/// at trigger time, held in `App::pending_materialize` between
/// `MaterializePrepare`'s ack (which carries no disk-sourced data of its
/// own) and the caller-side `vfs` `Cmd` it spawns. Never re-derived once
/// captured (§1.4.2/§1.4.8) — the eventual `vfs` work and `MaterializeRecord`
/// enqueue both read only from this struct, never from the document's
/// (possibly further-edited) live buffer.
#[derive(Clone)]
pub(crate) struct PendingMaterialize {
    content: String,
    path: PathBuf,
    bind_new: bool,
    db_id: i64,
    seq: i64,
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
            app.db_ops.insert(op_id, id);
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
            on_store_failure(app, e.to_string());
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
            app.db_ops.insert(op_id, id);
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
            on_store_failure(app, e.to_string());
        }
    }
}

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

/// What the caller-side `vfs` work ([`run_materialize_vfs`]) concluded —
/// every disk-sourced fact [`handle_materialize_vfs_done`] needs, carried
/// so this module never has to call `vfs` a second time to re-derive any
/// of it.
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
        let outcome = run_materialize_vfs(
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

/// The `vfs` dance itself, factored out of [`materialize_vfs_cmd`] so it is
/// plain, synchronous, testable logic. Mirrors the steps the pre-WP7
/// `rune-db::materialize`/`materialize_overwrite`/`materialize_create` used
/// to run on the writer thread, verbatim in shape, just against the
/// CALLER's own `vfs` instead.
fn run_materialize_vfs(
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
    let _ = vfs.remove(&temp);
    MaterializeVfsOutcome::Committed {
        data: data.to_vec(),
        stat,
        resolved_path: resolved,
    }
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
            app.db_ops.insert(op_id, id);
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
        doc.save_in_flight = false;
        doc.save_pending_version = None;
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

/// Bumps `id`'s snapshot-autosave generation and (re)schedules its 2s
/// debounce timer (plan WP5.S6, port of `workspace_timers.go:11`) — called
/// once per message batch that mutated the ACTIVE document's journal, from
/// `app::update`'s wrapper.
pub(crate) fn schedule_snapshot_debounce(app: &mut App, id: DocumentId, effects: &mut Effects) {
    if app.db.is_none() {
        return;
    }
    let Some(doc) = app.doc_mut(id) else { return };
    let Some(doc_db) = doc.db.as_mut() else {
        return;
    };
    doc_db.snapshot_generation = doc_db.snapshot_generation.wrapping_add(1);
    let generation = doc_db.snapshot_generation;
    effects.cmds.push(snapshot_timeout_cmd(id, generation));
}

fn snapshot_timeout_cmd(id: DocumentId, generation: u32) -> Cmd {
    Cmd::new(CmdKind::SnapshotDebounce, move || {
        std::thread::sleep(SNAPSHOT_DEBOUNCE);
        Some(Msg::SnapshotDue { id, generation })
    })
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
