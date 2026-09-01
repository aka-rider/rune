use crate::app::App;
use crate::document::DocumentId;
use crate::messages;

pub(super) fn handle_committed_ack(
    app: &mut App,
    id: DocumentId,
    pending_version: Option<u64>,
    saved: Option<&rune_db::Observation>,
    raced: bool,
) {
    if let Some(path) = app
        .doc_mut(id)
        .and_then(crate::document::Document::take_bind_target)
    {
        app.rebind_document_path(id, path);
        if app.active == id {
            let name = app.doc(id).map(crate::title::name_for).unwrap_or_default();
            app.title.seed(&name);
        }
    }
    let db_id = app.doc_db_id(id);
    if let Some(doc_db) = app.doc_mut(id).and_then(|d| d.doc_db_mut()) {
        doc_db.publish_mode = crate::db::PublishMode::OverwriteExisting;
    }
    if let Some(binding) = app.doc_file_binding_mut(id) {
        binding.baseline_epoch = binding.baseline_epoch.wrapping_add(1);
    }
    if let Some(saved) = saved
        && let Some(binding) = app.doc_file_binding_mut(id)
    {
        binding.expect_obs = Some(saved.id);
        binding.pending_rebaseline_hash = None;
    }
    let was_hardlinked = app.doc(id).is_some_and(|d| d.nlink.is_some_and(|n| n > 1));
    if was_hardlinked {
        messages::info(
            app,
            "saved \u{2014} this file was hard-linked; its other names still hold the previous content",
        );
    }
    if let Some(doc) = app.doc_mut(id) {
        doc.nlink = saved.and_then(|o| o.nlink);
    }
    if let Some(doc) = app.doc_mut(id) {
        match pending_version {
            Some(version) => {
                doc.finish_save_ok(version);
            }
            None => doc.abandon_save(),
        }
    }
    if saved.is_none() {
        reestablish_baseline_or_detach(app, id, db_id);
    }
    if raced {
        messages::info(
            app,
            "saved \u{2014} a concurrent external change was overwritten; its bytes were preserved",
        );
    }
}

fn reestablish_baseline_or_detach(app: &mut App, id: DocumentId, db_id: Option<i64>) {
    let path = app.doc(id).and_then(|d| d.resolved_path().cloned());
    let store_usable = app.db.as_ref().is_some_and(|db| !db.degraded);
    let re_baselined = match (path, store_usable) {
        (Some(path), true) => crate::db_enqueue::load_document_best_effort(app, id, &path),
        _ => false,
    };
    let still_usable = app.db.as_ref().is_some_and(|db| !db.degraded);
    if !re_baselined || !still_usable {
        if let Some(doc) = app.doc_mut(id) {
            doc.replica = crate::document::Replica::Detached;
        }
        if let Some(db_id) = db_id {
            app.prune_file_binding(db_id);
        }
    }
}
