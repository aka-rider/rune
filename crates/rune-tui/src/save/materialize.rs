//! The store-backed materialize dance: enqueue `MaterializePrepare`,
//! bookkeeping only; once `materialize_ack::handle_prepare_ack` reacts to
//! its ack, it spawns the caller-side `vfs` `Cmd` that runs
//! [`run_materialize_vfs`] — the ENTIRE `vfs` dance, through THIS app's own
//! `Vfs` handle, never the writer thread's, since a dead writer thread must
//! never make saving impossible, and the snapshot-autosave
//! debounce. Split out of `save.rs` to stay under the 500-line budget — that file
//! owns `trigger_save`'s start/refusal ladder and calls into
//! [`materialize_now`]/[`bind_new_now`] here.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rune_vfs::{PutCondition, PutOutcome, Vfs};

use crate::app::App;
use crate::document::{DocumentId, PublishParams};
use crate::materialize_ack::{self, MaterializeVfsOutcome};
use crate::runtime::{Effects, Msg, TimerKey};
use crate::save::SaveMode;
use crate::save::gate::SaveClearance;

/// The snapshot-autosave debounce window.
const SNAPSHOT_DEBOUNCE: Duration = Duration::from_secs(2);

/// Document-owned (`SaveState::Preparing`): enqueues
/// `MaterializePrepare` — a plain, non-blocking channel send (never I/O that
/// leaves this thread; the writer thread's reply carries no disk-sourced
/// data at all, only DB bookkeeping), so `update` can call it directly.
/// `content`/`path`/`seq`/`publish_mode` are all captured HERE, synchronously,
/// into `Document::begin_prepare` — never re-derived once the round trip is
/// under way, and never overwritable by a second attempt while this one's
/// own ticket is still live (`trigger_save`'s in-flight refusal).
pub(super) fn materialize_now(
    app: &mut App,
    id: DocumentId,
    path: PathBuf,
    version: u64,
    mode: SaveMode,
    clearance: &SaveClearance,
    effects: &mut Effects,
) {
    let Some(doc) = app.doc(id) else { return };
    let Some((db_id, last_known_seq, publish_mode)) = doc
        .doc_db()
        .map(|d| (d.db_id, d.last_known_seq, d.publish_mode))
    else {
        return;
    };
    let content: Arc<str> = Arc::from(doc.buffer.content());
    // The CAS baseline is shared per file, not per document — read from
    // `App::file_bindings`, never from a per-`Document` copy, so
    // this save compares against whatever the LAST save on this file
    // (whichever tab made it) actually advanced the baseline to. `install_or_join_file_binding`
    // is joined synchronously the instant a document installs its own
    // `DocDb` (`db_ack::handle_load_ack`/`handle_create_scratch_ack`), so a
    // document that reaches here `Bound` but with no matching entry is
    // an internal inconsistency, not an ordinary "first save" case — refuse
    // the coordinated path outright rather than guess a CAS baseline with a
    // sentinel, taking the exact same uncoordinated fallback an enqueue
    // failure below takes, with its own explicit message.
    let Some(binding) = app.doc_file_binding(id) else {
        materialize_ack::on_store_failure(
            app,
            &format!("materialize: document {id:?} bound to db_id {db_id} has no file binding"),
        );
        save_directly(app, id, path, version, content, clearance, effects);
        return;
    };
    let expect_obs = binding.expect_obs;
    let baseline_epoch = binding.baseline_epoch;
    let pending_rebaseline_hash = binding
        .pending_rebaseline_hash
        .clone()
        .map(rune_db::BlobHash);
    let Some(target) = publish_mode.materialize_target(expect_obs) else {
        materialize_ack::on_store_failure(
            app,
            &format!("materialize: document {id:?} bound to db_id {db_id} has no CAS baseline"),
        );
        save_directly(app, id, path, version, content, clearance, effects);
        return;
    };
    crate::db_enqueue::flush_pending_rebase(app, id);
    let Some(db) = app.db.as_ref() else { return };
    let result =
        db.store
            .materialize_prepare(rune_db::DocId(db_id), target, pending_rebaseline_hash);

    match result {
        Ok(op_id) => {
            app.db_ops
                .insert(op_id, crate::db::PendingOp::prepare(id, baseline_epoch));
            if let Some(doc) = app.doc_mut(id) {
                doc.begin_prepare(
                    version,
                    Arc::clone(&content),
                    PublishParams {
                        path,
                        publish_mode,
                        db_id,
                        seq: last_known_seq.0,
                        mode,
                        bind_target: None,
                    },
                    op_id,
                );
            }
        }
        Err(e) => {
            // Nothing has touched disk yet — the store couldn't even
            // perform the bookkeeping-only prepare step. Degrade the store
            // (same signal `on_store_failure` always raised on an
            // enqueue-time error) AND fall back to the uncoordinated
            // direct-vfs write, exactly like a document with no store
            // binding at all — save-anyway must actually save
            // even when the store is unavailable. This document never entered
            // `Preparing`, so `on_store_failure`'s sweep has nothing of
            // THIS attempt to abandon — the fallback below is the first
            // and only thing that arms `save_in_flight` for it.
            materialize_ack::on_store_failure(app, &e.to_string());
            save_directly(app, id, path, version, content, clearance, effects);
        }
    }
}

pub(super) fn save_directly(
    app: &mut App,
    id: DocumentId,
    path: PathBuf,
    version: u64,
    content: Arc<str>,
    _clearance: &SaveClearance,
    effects: &mut Effects,
) {
    let bytes = content.as_bytes().to_vec();
    let Some(doc) = app.doc_mut(id) else { return };
    let ticket = doc.begin_save(version, content);
    let vfs = Arc::clone(&app.vfs);
    effects
        .cmds
        .push(super::save_cmd(id, ticket, vfs, path, bytes, version));
}

/// The draft-naming route (`rename::bind_new`): materialize the buffer to
/// `path` create-only — an atomic no-clobber `rename_excl` create
/// whose EEXIST branch refuses and records the winner's bytes.
///
/// `trigger_save` cannot be reused here: it reads `doc.file_path`, which is
/// exactly what a draft does not have yet. And the document is deliberately
/// NOT bound to `path` up front — a `rename_excl` that loses the race must
/// leave the draft untitled, or a later ^S would overwrite the winner.
/// `handle_materialize_ack` performs the bind once the
/// write actually commits.
pub(crate) fn bind_new_now(
    app: &mut App,
    id: DocumentId,
    path: PathBuf,
    _clearance: &SaveClearance,
) {
    crate::commands::strip_trailing::leave_reading_then_strip(app, id);
    let Some(doc) = app.doc(id) else { return };
    let version = doc.buffer.version();
    let Some(db_id) = doc.doc_db().map(|d| d.db_id) else {
        return;
    };
    let content: Arc<str> = Arc::from(doc.buffer.content());
    crate::db_enqueue::flush_pending_rebase(app, id);
    let Some(db) = app.db.as_ref() else { return };
    // `expect`/CAS never applies on the create path — `prepare_materialize`
    // returns `MaterializePrep::Create` for `BindNew` and the caller-side
    // `vfs` work skips the read/hash-compare accordingly.
    let seq = app
        .doc(id)
        .and_then(|d| d.doc_db())
        .map_or(0, |d| d.last_known_seq.0);
    let result = db.store.materialize_prepare(
        rune_db::DocId(db_id),
        rune_db::MaterializeTarget::BindNew,
        None,
    );

    match result {
        Ok(op_id) => {
            app.db_ops.insert(op_id, crate::db::PendingOp::new(id));
            if let Some(doc) = app.doc_mut(id) {
                doc.begin_prepare(
                    version,
                    Arc::clone(&content),
                    PublishParams {
                        path: path.clone(),
                        publish_mode: crate::db::PublishMode::CreateOnly,
                        db_id,
                        seq,
                        // A create never reaches the mode-dependent branch —
                        // the create path has no CAS baseline to compare
                        // either way.
                        mode: SaveMode::Normal,
                        bind_target: Some(path),
                    },
                    op_id,
                );
            }
        }
        Err(e) => {
            // Unlike `materialize_now`'s overwrite path, there is no
            // equivalent-safety direct-vfs fallback for a bind-new create:
            // a plain `vfs.save_atomic` has no no-clobber guarantee, and
            // `handle_save_done`'s success path never binds `file_path`
            // (only `handle_materialize_ack`'s bind-target dance does) —
            // reusing it here would silently create the file without ever
            // giving the draft its name. The buffer itself is never at risk
            // (still safely in memory, `saved_version` untouched), just this
            // ONE draft-naming attempt; the user can retry once a fresh
            // store is available. This document never entered `Preparing`,
            // so there is nothing of THIS attempt for `on_store_failure`'s
            // state-aware handling to touch.
            materialize_ack::on_store_failure(app, &e.to_string());
        }
    }
}

/// The `vfs` dance itself, factored out of `materialize_vfs_cmd` so it is
/// plain, synchronous, testable logic: the pre-checks (path disagreement,
/// destination resolve) followed by one `rune_vfs::put` — `IfMatch` for an
/// ordinary compare-and-swap save, `Force` for the disk-conflict Guard's
/// escape hatch, `IfAbsent` for a create-only publish — and the adapter
/// mapping `PutOutcome` onto [`MaterializeVfsOutcome`].
pub(crate) fn run_materialize_vfs(
    vfs: &dyn Vfs,
    path: &Path,
    publish_mode: crate::db::PublishMode,
    content: &str,
    expect_hash: &str,
    bound_path: Option<&str>,
    mode: SaveMode,
) -> MaterializeVfsOutcome {
    let data = content.as_bytes();

    let resolved = match vfs.resolve(path) {
        Ok(r) => r,
        Err(e) => return MaterializeVfsOutcome::Error(e.to_string()),
    };

    if publish_mode.is_create_only() {
        if let Some(dir) = resolved.parent()
            && !dir.as_os_str().is_empty()
            && let Err(e) = vfs.mkdir_all(dir)
        {
            return MaterializeVfsOutcome::Error(e.to_string());
        }
        let outcome = rune_vfs::put(vfs, &resolved, data, PutCondition::IfAbsent);
        return map_put_outcome(outcome, data, resolved);
    }

    if let Some(bound) = bound_path {
        match vfs.resolve(Path::new(bound)) {
            Ok(db_resolved) if db_resolved != resolved => {
                return MaterializeVfsOutcome::PathDisagreement;
            }
            Ok(_) => {}
            Err(e) => return MaterializeVfsOutcome::Error(e.to_string()),
        }
    }

    let condition = match mode {
        SaveMode::Normal => match rune_vfs::Etag::from_stored(expect_hash) {
            Ok(etag) => PutCondition::IfMatch(etag),
            Err(e) => return MaterializeVfsOutcome::Error(e.to_string()),
        },
        SaveMode::Force => {
            let expect = if expect_hash.is_empty() {
                None
            } else {
                match rune_vfs::Etag::from_stored(expect_hash) {
                    Ok(etag) => Some(etag),
                    Err(e) => return MaterializeVfsOutcome::Error(e.to_string()),
                }
            };
            PutCondition::Force { expect }
        }
    };
    let outcome = rune_vfs::put(vfs, &resolved, data, condition);
    map_put_outcome(outcome, data, resolved)
}

fn map_put_outcome(
    outcome: std::io::Result<PutOutcome>,
    data: &[u8],
    resolved_path: PathBuf,
) -> MaterializeVfsOutcome {
    match outcome {
        Ok(PutOutcome::Missing) => MaterializeVfsOutcome::Missing,
        Ok(PutOutcome::Conflict { current, .. }) => MaterializeVfsOutcome::Conflict {
            data: current.bytes,
            origin: rune_db::ObsOrigin::Probe,
            stat: rune_db::stat_facts_from(current.sighted.stat()),
            confirmed: current.sighted.is_confirmed(),
            resolved_path,
        },
        Ok(PutOutcome::Committed {
            sighted,
            durable,
            stray_temp,
            ..
        }) => MaterializeVfsOutcome::Committed {
            data: data.to_vec(),
            confirmed: sighted.is_confirmed(),
            stat: rune_db::stat_facts_from(sighted.stat()),
            resolved_path,
            durable,
            stray_temp,
        },
        Ok(PutOutcome::Raced {
            sighted,
            durable,
            displaced,
            stray_temp,
            ..
        }) => MaterializeVfsOutcome::Raced {
            data: data.to_vec(),
            confirmed: sighted.is_confirmed(),
            stat: rune_db::stat_facts_from(sighted.stat()),
            displaced: displaced.bytes,
            stray_temp,
            displaced_stat: rune_db::stat_facts_from(displaced.sighted.stat()),
            resolved_path,
            durable,
        },
        Err(e) => MaterializeVfsOutcome::Error(e.to_string()),
    }
}

/// Bumps `id`'s snapshot-autosave generation and (re)arms its 2s debounce
/// deadline on `app`'s one rearmable timer thread (`App::timers` — see that
/// type's own doc comment) — called once per message batch that mutated the
/// ACTIVE document's journal, from `app::update`'s wrapper. No `Effects`
/// involved any more: arming the timer is a direct, synchronous call, not
/// I/O that needs a spawned thread of its own.
pub(crate) fn schedule_snapshot_debounce(app: &mut App, id: DocumentId) {
    if app.db.is_none() {
        return;
    }
    let Some(doc) = app.doc_mut(id) else { return };
    let Some(doc_db) = doc.doc_db_mut() else {
        return;
    };
    doc_db.snapshot_generation = doc_db.snapshot_generation.wrapping_add(1);
    let generation = doc_db.snapshot_generation;
    app.timers.arm(
        TimerKey::Snapshot(id),
        SNAPSHOT_DEBOUNCE,
        Msg::SnapshotDue { id, generation },
    );
}

// Kept in a sibling file: this module's own vfs dance stays under the
// 500-line budget on its own merits.
#[cfg(test)]
#[path = "materialize_tests.rs"]
mod tests;
