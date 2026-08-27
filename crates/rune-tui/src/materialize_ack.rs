use std::path::Path;

use rune_db::{MatResult, MaterializeOutcome, SyncKind};

use crate::app::App;
use crate::document::{DocumentId, SavePhase};
use crate::guard::{self, GuardKind, GuardPrompt};
use crate::messages;
use crate::runtime::Effects;

mod publish;
mod reactions;
pub use publish::MaterializeVfsOutcome;
pub(crate) use publish::{handle_materialize_vfs_done, handle_prepare_ack};
use reactions::fail_materialize_locally;
pub(crate) use reactions::{handle_materialize_ack, handle_save_done, retire_quit_wait};

pub(crate) const DURABILITY_UNCONFIRMED_WARNING: &str =
    "saved \u{2014} durability unconfirmed; prior content kept at the sibling temp";

pub(crate) fn stray_temp_warning(temp: &Path) -> String {
    format!(
        "saved, but a leftover temp file could not be removed: {}",
        temp.display()
    )
}

#[derive(Debug)]
pub enum SaveRace {
    Preserved(std::path::PathBuf),
    PreserveFailed(String),
}

pub(crate) fn race_preserved_message(path: &Path) -> String {
    format!(
        "saved \u{2014} content already on disk was replaced; the previous version was kept at {}",
        path.display()
    )
}

pub(crate) fn race_preserve_failed_warning(reason: &str) -> String {
    format!(
        "saved \u{2014} content already on disk was replaced, but the previous version could not be preserved: {reason}"
    )
}

pub(crate) const SAVE_REFUSED_DISK_CHANGED: &str =
    "save refused \u{2014} the file changed on disk since it was opened";

pub(crate) fn seed_refusal_classification(app: &mut App, id: DocumentId, kind: SyncKind) {
    if let Some(doc) = app.doc_mut(id) {
        doc.last_sync = Some(kind);
    }
}

pub(crate) fn raise_disk_conflict(
    app: &mut App,
    id: DocumentId,
    kind: SyncKind,
    effects: &mut Effects,
) {
    seed_refusal_classification(app, id, kind);
    let _ = guard::set_guard_or_warn(
        app,
        GuardPrompt {
            doc: id,
            kind: GuardKind::DiskConflict,
        },
        "disk-conflict confirmation dropped \u{2014} a prompt is already showing",
        effects,
    );
}

pub(crate) fn handle_materialize_ack_for_op(
    app: &mut App,
    id: DocumentId,
    op_id: u64,
    mat: MatResult,
    effects: &mut Effects,
) {
    if app.doc(id).and_then(super::document::Document::record_op) != Some(op_id) {
        return;
    }
    handle_materialize_ack(app, id, &mat, effects);
}

#[derive(Clone, Copy)]
struct RecordTarget<'a> {
    db_id: i64,
    seq: i64,
    content: &'a str,
    resolved_path: &'a Path,
}

fn record_outcome(
    app: &mut App,
    id: DocumentId,
    target: RecordTarget<'_>,
    outcome: MaterializeOutcome,
    published: bool,
) {
    let Some(db) = app.db.as_ref() else {
        if published {
            reactions::resolve_committed_ack(app, id);
        } else {
            fail_materialize_locally(app, id, "save failed: recovery store unavailable");
        }
        return;
    };
    match db.store.materialize_record(
        rune_db::DocId(target.db_id),
        target.resolved_path,
        target.seq,
        outcome,
    ) {
        Ok(op_id) => match app
            .doc_mut(id)
            .map(|doc| doc.begin_recording(op_id, published))
        {
            Some(true) => {
                app.db_ops.insert(op_id, crate::db::PendingOp::new(id));
            }
            Some(false) => {
                fail_materialize_locally(
                    app,
                    id,
                    "save failed: the write was recorded but the document's own save state had already moved on",
                );
            }
            None => {}
        },
        Err(e) => {
            if published {
                if let Some(binding) = app.doc_file_binding_mut(id) {
                    binding.pending_rebaseline_hash =
                        Some(rune_db::hash_bytes(target.content.as_bytes()));
                }
                reactions::resolve_committed_ack(app, id);
            } else {
                fail_materialize_locally(app, id, format!("save failed: {e}"));
            }
            on_store_failure(app, &e.to_string());
        }
    }
}

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
        .materialize_record(rune_db::DocId(db_id), resolved_path, seq, outcome);
}

pub(crate) fn on_store_failure(app: &mut App, error: &str) {
    if let Some(db) = app.db.as_mut() {
        db.degraded = true;
    }
    app.db_banner = Some(format!("recovery disabled: {error}"));
    messages::error(app, format!("recovery disabled: {error}"));

    let ids: Vec<DocumentId> = app.documents.keys().copied().collect();
    let mut abandoned_any = false;
    let mut resolved_committed = Vec::new();
    for id in &ids {
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
                let pending_version = doc.pending_save_version();
                if let Some(doc) = app.doc_mut(id) {
                    doc.abandon_save();
                }
                reactions::resolve_continuations(app, id, pending_version, false);
                abandoned_any = true;
            }
            SavePhase::Recording { published: true } => {
                resolved_committed.push(id);
            }
            SavePhase::Idle | SavePhase::Direct | SavePhase::Publishing => {}
        }
    }
    for id in resolved_committed {
        reactions::resolve_committed_ack(app, id);
    }
    if abandoned_any {
        messages::error(app, format!("save failed: {error}"));
    }

    app.quit = crate::app::QuitNegotiation::Idle;
}

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
    crate::db_enqueue::flush_pending_rebase(app, id);
    let Some(db) = app.db.as_ref() else { return };
    let result = db.store.create_snapshot(rune_db::DocId(db_id), &content);
    match result {
        Ok(op_id) => {
            app.db_ops.insert(op_id, crate::db::PendingOp::new(id));
        }
        Err(e) => on_store_failure(app, &e.to_string()),
    }
}
