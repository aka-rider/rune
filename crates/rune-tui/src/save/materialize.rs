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

const SNAPSHOT_DEBOUNCE: Duration = Duration::from_secs(2);

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

pub(crate) fn bind_new_now(
    app: &mut App,
    id: DocumentId,
    path: crate::resolved::ResolvedPath,
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
                        path: path.clone().into_path_buf(),
                        publish_mode: crate::db::PublishMode::CreateOnly,
                        db_id,
                        seq,
                        mode: SaveMode::Normal,
                        bind_target: Some(path),
                    },
                    op_id,
                );
            }
        }
        Err(e) => {
            // No direct-vfs fallback here: unlike an overwrite, a plain save
            // has no no-clobber guarantee, so it would silently create the
            // file without ever giving the draft its name.
            materialize_ack::on_store_failure(app, &e.to_string());
        }
    }
}

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
        return wrap_put_outcome(outcome, resolved);
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
    wrap_put_outcome(outcome, resolved)
}

fn wrap_put_outcome(
    outcome: std::io::Result<PutOutcome>,
    resolved_path: PathBuf,
) -> MaterializeVfsOutcome {
    match outcome {
        Ok(outcome) => MaterializeVfsOutcome::Put {
            resolved_path,
            outcome: Box::new(outcome),
        },
        Err(e) => MaterializeVfsOutcome::Error(e.to_string()),
    }
}

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

#[cfg(test)]
#[path = "materialize_tests/mod.rs"]
mod tests;
