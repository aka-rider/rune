use rune_db::MatResult;

use crate::app::App;
use crate::document::DocumentId;
use crate::messages;
use crate::runtime::{CmdError, Effects};
use crate::workspace;

use super::SaveRace;

#[path = "committed.rs"]
mod committed;
use committed::handle_committed_ack;

pub(crate) fn fail_materialize_locally(app: &mut App, id: DocumentId, message: impl Into<String>) {
    let pending_version = app
        .doc(id)
        .and_then(super::super::document::Document::pending_save_version);
    if let Some(doc) = app.doc_mut(id) {
        doc.abandon_save();
    }
    messages::error(app, message.into());
    resolve_continuations(app, id, pending_version, false);
}

fn lost_create_race(app: &App, id: DocumentId) -> Option<crate::resolved::ResolvedPath> {
    let doc = app.doc(id)?;
    if !doc
        .doc_db()
        .is_some_and(|d| d.publish_mode.is_create_only())
    {
        return None;
    }
    if doc.bind_target().is_some() {
        return None;
    }
    doc.resolved_path().cloned()
}

fn naming_collision(app: &App, id: DocumentId) -> Option<crate::resolved::ResolvedPath> {
    let doc = app.doc(id)?;
    if !doc
        .doc_db()
        .is_some_and(|d| d.publish_mode.is_create_only())
    {
        return None;
    }
    doc.bind_target().cloned()
}

pub(crate) fn handle_materialize_ack(
    app: &mut App,
    id: DocumentId,
    mat: &MatResult,
    effects: &mut Effects,
) {
    let Some(doc) = app.doc(id) else { return };
    let pending_version = doc.pending_save_version();
    let committed = matches!(
        mat,
        MatResult::Committed { .. } | MatResult::CommittedRaced { .. }
    );

    match mat {
        MatResult::Committed { saved } => {
            handle_committed_ack(app, id, pending_version, saved.as_ref(), false);
        }
        MatResult::CommittedRaced { saved, .. } => {
            handle_committed_ack(app, id, pending_version, Some(saved), true);
        }
        MatResult::Missing => abandon_missing(app, id),
        MatResult::Refused { .. } => {
            handle_refused_ack(app, id, effects);
        }
    }
    finish_ack(app, id, pending_version, committed);
}

fn abandon_missing(app: &mut App, id: DocumentId) {
    if let Some(doc) = app.doc_mut(id) {
        doc.abandon_save();
    }
    messages::error(app, "save failed: file no longer exists");
}

pub(crate) fn resolve_missing_ack(app: &mut App, id: DocumentId) {
    let Some(doc) = app.doc(id) else { return };
    let pending_version = doc.pending_save_version();
    abandon_missing(app, id);
    finish_ack(app, id, pending_version, false);
}

pub(crate) fn resolve_committed_ack(app: &mut App, id: DocumentId) {
    let Some(doc) = app.doc(id) else { return };
    let pending_version = doc.pending_save_version();
    handle_committed_ack(app, id, pending_version, None, false);
    finish_ack(app, id, pending_version, true);
}

fn finish_ack(app: &mut App, id: DocumentId, pending_version: Option<u64>, committed: bool) {
    let db_id = app.doc_db_id(id);
    let deferred_probe = app
        .doc_file_binding_mut(id)
        .is_some_and(|binding| std::mem::take(&mut binding.pending_probe));
    if deferred_probe && let Some(db_id) = db_id {
        for doc_id in app.documents_bound_to(db_id) {
            let _ = crate::db_enqueue::probe(app, doc_id);
        }
    }
    resolve_continuations(app, id, pending_version, committed);
}

fn handle_refused_ack(app: &mut App, id: DocumentId, effects: &mut Effects) {
    let race = lost_create_race(app, id);
    let naming = naming_collision(app, id);
    if let Some(doc) = app.doc_mut(id) {
        doc.abandon_save();
    }
    if let Some(path) = race {
        let hand_off_safe = workspace::existing_document_for(app, &path) == Some(id);
        let can_hand_off = hand_off_safe && app.db.as_ref().is_some_and(|db| !db.degraded);
        if can_hand_off {
            messages::error(
                app,
                "save failed: the target was created by something else; your changes are unsaved \u{2014} ^M to merge",
            );
            super::seed_refusal_classification(app, id, rune_db::SyncKind::Diverged);
            let _ = crate::db_enqueue::load_document(
                app,
                id,
                &path,
                crate::db_enqueue::LoadIntent::Rebaseline,
            );
        } else {
            messages::error(
                app,
                "save failed: the target was created by something else; your buffer is intact \u{2014} ^R to a different name to save it",
            );
        }
    } else if let Some(target) = naming {
        let name = target.file_name().map_or_else(
            || target.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        messages::error(app, format!("{name} already exists"));
        app.refocus_title();
    } else {
        messages::error(app, super::SAVE_REFUSED_DISK_CHANGED);
        super::raise_disk_conflict(app, id, rune_db::SyncKind::Diverged, effects);
    }
}

pub(crate) fn handle_save_done(
    app: &mut App,
    id: DocumentId,
    ticket: crate::document::SaveTicket,
    version: u64,
    result: Result<(), CmdError>,
    detail: crate::runtime::SaveOutcomeDetail,
) {
    if app
        .doc(id)
        .and_then(super::super::document::Document::save_ticket)
        != Some(ticket)
    {
        return;
    }
    let succeeded = result.is_ok();
    match result {
        Ok(()) => {
            if let Some(doc) = app.doc_mut(id) {
                doc.finish_save_ok(version);
            }
            if !detail.durable {
                messages::warn(app, super::DURABILITY_UNCONFIRMED_WARNING);
            }
            if let Some(temp) = &detail.stray_temp {
                messages::warn(app, super::stray_temp_warning(temp));
            }
            match detail.race {
                Some(SaveRace::Preserved(path)) => {
                    messages::info(app, super::race_preserved_message(&path));
                }
                Some(SaveRace::PreserveFailed(reason)) => {
                    messages::warn(app, super::race_preserve_failed_warning(&reason));
                }
                None => {}
            }
        }
        Err(e) => {
            if let Some(doc) = app.doc_mut(id) {
                doc.abandon_save();
            }
            messages::error(app, format!("save failed: {e}"));
        }
    }
    resolve_continuations(app, id, Some(version), succeeded);
}

pub(super) fn resolve_continuations(
    app: &mut App,
    id: DocumentId,
    version: Option<u64>,
    succeeded: bool,
) {
    close_if_pending(app, id, succeeded);
    quit_if_pending(app, id, version, succeeded);
}

fn close_if_pending(app: &mut App, id: DocumentId, succeeded: bool) {
    if app.pending_close_on_save != Some(id) {
        return;
    }
    app.pending_close_on_save = None;
    if succeeded {
        let mut effects = Effects::default();
        let _ = workspace::close_now(app, id, &mut effects);
    }
}

fn quit_if_pending(app: &mut App, id: DocumentId, version: Option<u64>, succeeded: bool) {
    let Some(intent) = app.quit.fan_out() else {
        return;
    };
    if !intent.pending.contains_key(&id) {
        return;
    }
    if succeeded {
        if version == app.quit.fan_out().and_then(|i| i.pending.get(&id).copied()) {
            retire_quit_wait(app, id);
        }
    } else {
        app.quit = crate::app::QuitNegotiation::Idle;
    }
}

pub(crate) fn retire_quit_wait(app: &mut App, id: DocumentId) {
    let Some(intent) = app.quit.fan_out_mut() else {
        return;
    };
    if intent.pending.remove(&id).is_none() {
        return;
    }
    if intent.pending.is_empty() {
        app.quit = crate::app::QuitNegotiation::Idle;
        app.should_quit = true;
    }
}

#[cfg(test)]
#[path = "reactions_tests.rs"]
mod tests;
