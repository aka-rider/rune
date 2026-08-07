//! The store-backed materialize dance (WP7: enqueue `MaterializePrepare`,
//! bookkeeping only; once `materialize_ack::handle_prepare_ack` reacts to
//! its ack, it spawns the caller-side `vfs` `Cmd` that runs
//! [`run_materialize_vfs`] — the ENTIRE `vfs` dance, through THIS app's own
//! `Vfs` handle, never the writer thread's, since a dead writer thread must
//! never make saving impossible ([rune-db 1])), and the snapshot-autosave
//! debounce. Split out of `save.rs` (plan WP1, 500-line budget) — that file
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
use crate::save::SaveMode;

/// The snapshot-autosave debounce window (plan WP5.S6).
const SNAPSHOT_DEBOUNCE: Duration = Duration::from_secs(2);

/// WP7: the content/path/CAS facts `materialize_now`/`bind_new_now` capture
/// at trigger time, held in `App::pending_materialize` between
/// `MaterializePrepare`'s ack (which carries no disk-sourced data of its
/// own) and the caller-side `vfs` `Cmd` it spawns. Never re-derived once
/// captured — the eventual `vfs` work and `MaterializeRecord`
/// enqueue both read only from this struct, never from the document's
/// (possibly further-edited) live buffer.
#[derive(Clone)]
pub(crate) struct PendingMaterialize {
    pub(crate) content: String,
    pub(crate) path: PathBuf,
    pub(crate) bind_new: bool,
    pub(crate) db_id: i64,
    pub(crate) seq: i64,
    pub(crate) mode: SaveMode,
}

/// WP7 step (a): enqueues `MaterializePrepare` — a plain, non-blocking
/// channel send (never I/O that leaves this thread; the writer thread's
/// reply carries no disk-sourced data at all, only DB bookkeeping), so
/// `update` can call it directly. `content`/`path`/`seq`/`bind_new`
/// are all captured HERE, synchronously, into `App::pending_materialize` —
/// never re-derived once the round trip is under way.
/// `content` is captured through `Document::begin_save` (plan WP1's
/// chokepoint) BEFORE the match on `result`, so `save_in_flight` is never
/// true without the exact bytes this save will persist.
pub(super) fn materialize_now(
    app: &mut App,
    id: DocumentId,
    path: PathBuf,
    version: u64,
    mode: SaveMode,
    effects: &mut Effects,
) {
    let Some(doc) = app.doc(id) else { return };
    let Some((db_id, last_known_seq, bind_new)) = doc
        .db
        .as_ref()
        .map(|d| (d.db_id, d.last_known_seq, d.bind_new))
    else {
        return;
    };
    // The CAS baseline is shared per file, not per document (plan gap G7) —
    // read from `App::file_bindings`, never from a per-`Document` copy, so
    // this save compares against whatever the LAST save on this file
    // (whichever tab made it) actually advanced the baseline to.
    let expect_obs = app.file_binding(db_id).map(|b| b.expect_obs).unwrap_or(0);
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
                    mode,
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
/// leave the draft untitled, or a later ⌘S would overwrite the winner.
/// `handle_materialize_ack` performs the bind once the
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
                    // `bind_new` never reaches the mode-dependent branch —
                    // the create path has no CAS baseline to compare either
                    // way.
                    mode: SaveMode::Normal,
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

/// The bound on how many times the CAS check re-reads the live target while
/// it disagrees with `expect_hash` before treating the mismatch as a stable
/// conflict (plan Task 4) — a transient window (an external writer that
/// wrote then reverted inside the gap) must never raise the disk-conflict
/// guard from a single read.
const CAS_VERIFY_ATTEMPTS: u32 = 2;

/// The `vfs` dance itself, factored out of `materialize_vfs_cmd` so it is
/// plain, synchronous, testable logic. Mirrors the steps the pre-WP7
/// `rune-db::materialize`/`materialize_overwrite`/`materialize_create` used
/// to run on the writer thread, verbatim in shape, just against the
/// CALLER's own `vfs` instead. Every fresh read of the live target is
/// bracketed (`rune_db::bracketed_read`) — a racer caught mid-external-
/// rewrite must never become a trusted `Conflict` capture — and every stat
/// of OUR OWN just-published bytes is bracketed too (`rune_db::
/// bracketed_stat`), so a racer landing between the publish and the stat can
/// never let the resulting observation's blob and stat quietly disagree.
pub(crate) fn run_materialize_vfs(
    vfs: &dyn Vfs,
    path: &Path,
    bind_new: bool,
    content: &str,
    expect_hash: &str,
    bound_path: Option<&str>,
    mode: SaveMode,
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
                let bracket = rune_db::bracketed_stat(vfs, &resolved);
                MaterializeVfsOutcome::Committed {
                    data: data.to_vec(),
                    stat: bracket.stat,
                    confirmed: bracket.confirmed,
                    resolved_path: resolved,
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // A concurrent creator won the race — our own temp is
                // genuinely unneeded (the winner's bytes are what get
                // recorded), safe to discard.
                let _ = vfs.remove(&temp);
                match rune_db::bracketed_read(vfs, &resolved) {
                    Ok(bracket) => MaterializeVfsOutcome::Conflict {
                        data: bracket.data,
                        origin: "probe",
                        stat: bracket.stat,
                        confirmed: bracket.confirmed,
                        resolved_path: resolved,
                    },
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

    if mode == SaveMode::Force {
        return force_publish(vfs, &resolved, data);
    }

    // Step 1-2: bracketed read+hash of the live target, re-verified
    // (bounded) while it disagrees with `expect_hash` — a stable mismatch
    // after every attempt is a real conflict; a mismatch that stops
    // reproducing on a later attempt was a transient window, and the save
    // proceeds against the CURRENT live content instead of refusing it.
    let mut bracket = match rune_db::bracketed_read(vfs, &resolved) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // An ordinary overwrite-intent save must never silently
            // (re)create a file the caller didn't explicitly ask to.
            return MaterializeVfsOutcome::Missing;
        }
        Err(e) => return MaterializeVfsOutcome::Error(e.to_string()),
    };
    let mut attempts = 1;
    while rune_db::hash_bytes(&bracket.data) != expect_hash && attempts < CAS_VERIFY_ATTEMPTS {
        bracket = match rune_db::bracketed_read(vfs, &resolved) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return MaterializeVfsOutcome::Missing;
            }
            Err(e) => return MaterializeVfsOutcome::Error(e.to_string()),
        };
        attempts += 1;
    }
    if rune_db::hash_bytes(&bracket.data) != expect_hash {
        return MaterializeVfsOutcome::Conflict {
            data: bracket.data,
            origin: "probe",
            stat: bracket.stat,
            confirmed: bracket.confirmed,
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
    // bytes, never unlinked by the swap) — read+hash it. `temp` is our own
    // private scratch path, never contended, so a plain read/stat suffices;
    // only `resolved`'s own post-publish stat needs bracketing.
    let displaced = match vfs.read(&temp) {
        Ok(d) => d,
        Err(e) => return MaterializeVfsOutcome::Error(e.to_string()),
    };
    let stat_bracket = rune_db::bracketed_stat(vfs, &resolved);
    if rune_db::hash_bytes(&displaced) != expect_hash {
        // F5 swap-race: a writer raced us inside the atomic-swap window.
        let displaced_stat = rune_db::stat_identity(vfs, &temp);
        let _ = vfs.remove(&temp);
        return MaterializeVfsOutcome::Raced {
            data: data.to_vec(),
            stat: stat_bracket.stat,
            confirmed: stat_bracket.confirmed,
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
        stat: stat_bracket.stat,
        confirmed: stat_bracket.confirmed,
        resolved_path: resolved,
    }
}

/// `SaveMode::Force`'s publish: existence-aware (`exchange` when the
/// destination is there to swap with, `rename_excl` when it has vanished —
/// the same ladder `Vfs::save_atomic` composes, chosen explicitly here
/// instead of implicitly so the swap side can hand its displaced bytes on
/// rather than discard them) and never CAS-gated — there is no hash this
/// mode refuses on. Existence is read, not merely stated, because a
/// `RENAME_SWAP` needs both paths to exist: a blind `exchange` would fail
/// exactly when the user most needs the save to go through.
fn force_publish(vfs: &dyn Vfs, resolved: &Path, data: &[u8]) -> MaterializeVfsOutcome {
    let dest_existed = match vfs.read(resolved) {
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return MaterializeVfsOutcome::Error(e.to_string()),
    };

    let temp = match vfs.write_durable(resolved, data) {
        Ok(t) => t,
        Err(e) => return MaterializeVfsOutcome::Error(e.to_string()),
    };

    if !dest_existed {
        return match vfs.rename_excl(&temp, resolved) {
            Ok(()) => {
                let bracket = rune_db::bracketed_stat(vfs, resolved);
                MaterializeVfsOutcome::Committed {
                    data: data.to_vec(),
                    stat: bracket.stat,
                    confirmed: bracket.confirmed,
                    resolved_path: resolved.to_path_buf(),
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // A concurrent creator filled the destination between our
                // existence read and this publish — it genuinely exists
                // now, so swap onto it rather than reporting a failure the
                // user has no way to retry out of; the temp we already
                // wrote is still exactly what needs publishing.
                capture_and_swap_publish(vfs, resolved, &temp, data)
            }
            // Deliberately NOT removed: the temp is the only place the
            // user's just-written bytes still physically exist.
            Err(e) => MaterializeVfsOutcome::Error(e.to_string()),
        };
    }

    capture_and_swap_publish(vfs, resolved, &temp, data)
}

/// The swap half of [`force_publish`]: publishes via `exchange`, then reads
/// `temp` back — the swap deposits whatever WAS at `resolved` there, never
/// unlinking it — and reports that as displaced, unconditionally. No hash
/// gate anywhere in this function: whatever the swap actually displaced is
/// what gets captured, following the same unconditional-capture shape
/// `rune-db`'s own user-confirmed destructive rename uses.
fn capture_and_swap_publish(
    vfs: &dyn Vfs,
    resolved: &Path,
    temp: &Path,
    data: &[u8],
) -> MaterializeVfsOutcome {
    match vfs.exchange(temp, resolved) {
        Ok(()) => {}
        Err(e) if rune_vfs::published_not_durable(&e) => {
            // The swap already took effect; only the durability
            // confirmation failed. `resolved` already holds the new bytes —
            // keep going and capture what it displaced, same as the CAS
            // path's own handling of this error shape.
        }
        // Deliberately NOT removed: the temp is the only place the user's
        // just-written bytes still physically exist.
        Err(e) => return MaterializeVfsOutcome::Error(e.to_string()),
    }

    let displaced = match vfs.read(temp) {
        Ok(d) => d,
        Err(e) => return MaterializeVfsOutcome::Error(e.to_string()),
    };
    let stat_bracket = rune_db::bracketed_stat(vfs, resolved);
    let displaced_stat = rune_db::stat_identity(vfs, temp);
    // The displaced bytes are already read into `displaced` above; a failed
    // cleanup here just leaks a scratch file, never loses them.
    let _ = vfs.remove(temp);
    MaterializeVfsOutcome::Raced {
        data: data.to_vec(),
        stat: stat_bracket.stat,
        confirmed: stat_bracket.confirmed,
        displaced,
        displaced_stat,
        resolved_path: resolved.to_path_buf(),
    }
}

/// Bumps `id`'s snapshot-autosave generation and (re)arms its 2s debounce
/// deadline on `app`'s one rearmable timer thread (plan WP5.S6; plan
/// WP16.S5 replaced the previous per-call `Cmd` spawn with
/// `App::snapshot_timer` — see that type's own doc comment) — called
/// once per message batch that mutated the ACTIVE
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

// Kept in a sibling file: this module's own vfs dance stays under the
// 500-line budget on its own merits.
#[cfg(test)]
#[path = "materialize_tests.rs"]
mod tests;
