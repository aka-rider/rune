//! The store-backed materialize dance (WP7: enqueue `MaterializePrepare`,
//! bookkeeping only; once `materialize_ack::handle_prepare_ack` reacts to
//! its ack, it spawns the caller-side `vfs` `Cmd` that runs
//! [`run_materialize_vfs`] — the ENTIRE `vfs` dance, through THIS app's own
//! `Vfs` handle, never the writer thread's, since a dead writer thread must
//! never make saving impossible ([rune-db 1])), and the snapshot-autosave
//! debounce. Split out of `save.rs` (plan WP1, §1.6 budget) — that file
//! owns `trigger_save`'s start/refusal ladder and calls into
//! [`materialize_now`]/[`bind_new_now`] here.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rune_vfs::Vfs;

use crate::app::App;
use crate::document::DocumentId;
use crate::materialize_ack::{self, MaterializeVfsOutcome};
use crate::runtime::Effects;

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

/// WP7 step (a): enqueues `MaterializePrepare` — a plain, non-blocking
/// channel send (never I/O that leaves this thread; the writer thread's
/// reply carries no disk-sourced data at all, only DB bookkeeping), so
/// §5.4 lets `update` call it directly. `content`/`path`/`seq`/`bind_new`
/// are all captured HERE, synchronously, into `App::pending_materialize` —
/// never re-derived once the round trip is under way (§1.4.2/§1.4.8).
/// `content` is captured through `Document::begin_save` (plan WP1's
/// chokepoint) BEFORE the match on `result`, so `save_in_flight` is never
/// true without the exact bytes this save will persist.
pub(super) fn materialize_now(
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
    let content: Arc<str> = Arc::from(doc.buffer.content());
    let Some(db) = app.db.as_ref() else { return };
    let result = db.store.materialize_prepare(db_id, expect_obs, bind_new);

    if let Some(doc) = app.doc_mut(id) {
        doc.begin_save(version, Arc::clone(&content));
    }
    match result {
        Ok(op_id) => {
            app.db_ops.insert(op_id, crate::db::PendingOp::new(id));
            app.pending_materialize.insert(
                id,
                PendingMaterialize {
                    content: content.to_string(),
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
            // sweep abandons the save for EVERY document (it has no way to
            // know this ONE document's save is about to continue via the
            // fallback `Cmd`) — this one included — so it must be re-armed
            // right after with the SAME capture (plan WP1: not just the
            // `save_in_flight` flag), or the fallback `Cmd`'s eventual
            // `SaveDone` would have no `save_pending` left to promote from,
            // leaving the document dirty forever despite a successful save.
            materialize_ack::on_store_failure(app, e.to_string());
            if let Some(doc) = app.doc_mut(id) {
                doc.begin_save(version, Arc::clone(&content));
            }
            let bytes = content.as_bytes().to_vec();
            let vfs = Arc::clone(&app.vfs);
            effects
                .cmds
                .push(crate::save::save_cmd(id, vfs, path, bytes, version));
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
    let content: Arc<str> = Arc::from(doc.buffer.content());
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
        doc.begin_save(version, Arc::clone(&content));
        // Remembered so the ack can bind it — see `pending_bind_path`.
        doc.pending_bind_path = Some(path.clone());
    }
    match result {
        Ok(op_id) => {
            app.db_ops.insert(op_id, crate::db::PendingOp::new(id));
            app.pending_materialize.insert(
                id,
                PendingMaterialize {
                    content: content.to_string(),
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
                doc.abandon_save();
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
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
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
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
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
