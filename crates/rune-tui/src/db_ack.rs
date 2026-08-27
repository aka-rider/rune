use crate::app::App;
use crate::db::{DocDb, LoadPurpose, PublishMode};
use crate::document::{DocumentId, Replica, ReplicaStep};
use crate::messages;
use rune_core::buffer::AppliedEdit;
use rune_core::undo::EditKind;
#[cfg(test)]
use rune_db::BlobHash;
use rune_db::LoadResult;

pub fn handle_load_ack(
    app: &mut App,
    id: DocumentId,
    load_result: LoadResult,
    issued_version: Option<u64>,
    purpose: LoadPurpose,
) {
    let Some(expect_obs) = load_result.saved_obs else {
        detach_file_binding(app, id);
        messages::error(
            app,
            "crash recovery unavailable for this tab: load returned no baseline observation",
        );
        return;
    };

    if let LoadPurpose::Rebaseline { expect_row } = purpose {
        let loaded_row = load_result.doc_id.0;
        let writer_kept_numbering = expect_row == Some(loaded_row);
        let still_bound_there = app.doc_db_id(id) == Some(loaded_row);
        if writer_kept_numbering && !still_bound_there {
            messages::warn(
                app,
                "undo history for this tab may no longer be recoverable step by step \u{2014} save to settle it",
            );
            return;
        }
        app.rebaseline_file_binding(loaded_row, expect_obs);
        let mode = if writer_kept_numbering {
            BindMode::Preserved
        } else {
            BindMode::Restarted
        };
        let pending = install_doc_db(app, id, &load_result, mode);
        crate::db_enqueue::replay_pending(app, id, pending);
        return;
    }

    if app.merge.doc() == Some(id) {
        crate::merge::auto_exit(app);
    }
    let hydration = {
        let Some(doc) = app.doc_mut(id) else { return };
        if issued_version == Some(doc.buffer.version()) {
            Some(doc.hydrate(
                &load_result.disk_content,
                &load_result.recovered.content,
                &load_result.recovered.cursors,
            ))
        } else {
            None
        }
    };
    let adopted = matches!(&hydration, Some(crate::document::Hydration::Adopted));
    match hydration {
        Some(crate::document::Hydration::Refused(reason)) => {
            messages::error(app, format!("crash recovery: {reason}"));
            detach_file_binding(app, id);
            return;
        }
        Some(crate::document::Hydration::Adopted)
            if load_result.sync.kind == rune_db::SyncKind::Diverged
                && load_result.resumable_merge.is_none() =>
        {
            messages::warn(
                app,
                "recovered unsaved changes — the file on disk has changed since \u{21c4} [^M]erge to reconcile",
            );
        }
        Some(crate::document::Hydration::Adopted) => {
            messages::info(app, "recovered unsaved changes");
        }
        Some(crate::document::Hydration::NoChange) | None => {}
    }
    app.install_or_join_file_binding(load_result.doc_id.0, Some(expect_obs));
    let pending = install_doc_db(app, id, &load_result, BindMode::Restarted);
    crate::db_enqueue::replay_pending(app, id, pending);
    let Some(doc) = app.doc_mut(id) else { return };
    doc.last_sync = Some(load_result.sync.kind);
    doc.nlink = Some(load_result.nlink);
    warn_hard_links(app, load_result.nlink);
    if adopted && let Some(resume) = load_result.resumable_merge {
        crate::merge::resume_from_store(app, id, &resume.blocks_json, resume.theirs_obs);
    }
}

fn install_doc_db(
    app: &mut App,
    id: DocumentId,
    load_result: &LoadResult,
    mode: BindMode,
) -> Vec<ReplicaStep> {
    let doc_db = DocDb::new(
        load_result.doc_id.0,
        PublishMode::OverwriteExisting,
        load_result.bridge_seq.unwrap_or(rune_db::Seq(0)),
    );
    bind_document_row(app, id, doc_db, &load_result.recovered.content, mode)
}

#[derive(Clone, Copy)]
enum BindMode {
    Restarted,
    Preserved,
}

struct PriorBinding {
    db_id: i64,
    last_known_seq: rune_db::Seq,
    pending_rebase: Option<ReplicaStep>,
    token: rune_db::BindingToken,
    token_base_seq: rune_db::Seq,
    undo_offset: i64,
    undo_floor: i64,
    diverged: bool,
    synced_content: String,
}

/// The one chokepoint every fresh `Replica::Bound` install goes through.
/// `row_content` is what the bound row's durable journal reconstructs to
/// right now — every bind refreshes the shared `FileBinding::shared_content`
/// to it.
///
/// `doc_db` already carries a fresh `BindingToken`; this function decides
/// whether to keep it.
///
/// `Preserved` (a same-row re-baseline that touched neither the buffer nor
/// the durable journal) carries the prior `DocDb`'s whole mapping across
/// instead, discarding the fresh token: the local journal gained nothing
/// across such a load, so switching tokens would abandon a numbering the
/// writer still holds and can resolve exactly.
///
/// `Restarted` on the SAME row keeps the fresh token and sets
/// `diverged`/`synced_content` from a direct comparison against
/// `row_content` — no bridge is journaled, so `undo_offset`/`undo_floor`
/// are the trivial identity: position `0` under the fresh token already IS
/// `row_content`.
///
/// Binding to a DIFFERENT row must re-base the writer-side replica, or the
/// very next `AppendEdit` replays buffer-coordinate edits against content
/// of the wrong length and recovery dies with an out-of-bounds replay.
/// When the content the pending window's coordinates assume differs from
/// `row_content`, one synthetic replace-all bridge is computed and
/// DEFERRED onto `pending_rebase` rather than journaled eagerly — a
/// never-edited re-baseline bind must leave the row's reconstruction
/// intact. `undo_floor` becomes `1`, since that bridge, once flushed,
/// occupies the fresh token's own first entry, one ahead of position `0`.
/// The window replays verbatim only while it still mirrors the whole local
/// journal; a journal that moved underneath it (an undo inside the window,
/// or a window opened over an existing journal) makes its coordinates
/// unreplayable, so it is subsumed into a single bridge to the live buffer
/// instead — flushed eagerly, because the subsumed keystrokes exist
/// nowhere durable until it lands. The prior row's now-unreferenced
/// `FileBinding` is pruned.
fn bind_document_row(
    app: &mut App,
    id: DocumentId,
    mut doc_db: DocDb,
    row_content: &str,
    mode: BindMode,
) -> Vec<ReplicaStep> {
    let new_db_id = doc_db.db_id;
    app.set_shared_content(new_db_id, row_content);
    let Some(doc) = app.doc_mut(id) else {
        return Vec::new();
    };
    let prior = doc.doc_db_mut().map(|db| PriorBinding {
        db_id: db.db_id,
        last_known_seq: db.last_known_seq,
        pending_rebase: db.pending_rebase.take(),
        token: db.token,
        token_base_seq: db.token_base_seq,
        undo_offset: db.undo_offset,
        undo_floor: db.undo_floor,
        diverged: db.diverged,
        synced_content: db.synced_content.clone(),
    });
    let window = doc.replica.take_window();
    let mut pending = window.pending;
    let pos = crate::db_enqueue::journal_i64(doc.journal.pos());
    match prior {
        Some(prior) if prior.db_id == new_db_id => {
            match mode {
                BindMode::Preserved => {
                    doc_db.token = prior.token;
                    doc_db.token_base_seq = prior.token_base_seq;
                    doc_db.undo_offset = prior.undo_offset;
                    doc_db.undo_floor = prior.undo_floor;
                    doc_db.diverged = prior.diverged;
                    doc_db.synced_content = prior.synced_content;
                }
                BindMode::Restarted => {
                    doc_db.diverged = doc.buffer.content() != row_content;
                    doc_db.synced_content = row_content.to_string();
                    doc_db.undo_offset = pos;
                    doc_db.undo_floor = 0;
                }
            }
            doc_db.last_known_seq = doc_db.last_known_seq.max(prior.last_known_seq);
            doc_db.pending_rebase = prior.pending_rebase;
            doc.replica = Replica::Bound(doc_db);
            pending
        }
        prior => {
            let window_intact = !pending.is_empty()
                && crate::db_enqueue::journal_i64(pending.len()) == pos
                && window.base.is_some();
            let flush_now = !pending.is_empty() && !window_intact;
            let base = match (window.base, window_intact) {
                (Some(base), true) => base,
                _ => {
                    pending.clear();
                    doc.buffer.content().to_string()
                }
            };
            let mut bridged = 0;
            if base != row_content {
                doc_db.pending_rebase = Some(ReplicaStep::new(
                    &[AppliedEdit {
                        start: 0,
                        end: row_content.len(),
                        deleted: row_content.to_string(),
                        insert: base,
                    }],
                    &[],
                    &[],
                    EditKind::Other,
                ));
                doc_db.undo_floor = 1;
                bridged = 1;
            }
            doc_db.synced_content = row_content.to_string();
            let replayed = crate::db_enqueue::journal_i64(pending.len());
            doc_db.undo_offset = pos - replayed - bridged;
            doc.replica = Replica::Bound(doc_db);
            if let Some(prior) = prior {
                app.prune_file_binding(prior.db_id);
            }
            if flush_now {
                crate::db_enqueue::flush_pending_rebase(app, id);
            }
            pending
        }
    }
}

fn detach_file_binding(app: &mut App, id: DocumentId) {
    let old_db_id = app.doc_db_id(id);
    if let Some(doc) = app.doc_mut(id) {
        doc.replica = Replica::Detached;
    }
    if let Some(db_id) = old_db_id {
        app.prune_file_binding(db_id);
    }
}

pub fn warn_hard_links(app: &mut App, nlink: i64) {
    if nlink > 1 {
        messages::warn(
            app,
            format!(
                "this file has {nlink} hard links \u{2014} saving replaces it atomically, so the other links keep the old content"
            ),
        );
    }
}

/// `id` no longer live (an ack racing a future close) is a correct, silent
/// drop — the document it would have updated is already gone.
pub fn resolve_append_ack(app: &mut App, id: DocumentId, seq: rune_db::Seq) {
    let Some(doc) = app.doc_mut(id) else { return };
    if let Some(doc_db) = doc.doc_db_mut() {
        doc_db.resolve_append_ack(seq);
    }
}

pub fn handle_create_scratch_ack(app: &mut App, id: DocumentId, row_id: i64) {
    bind_scratch_doc(app, id, row_id);
}

pub fn bind_scratch_doc(app: &mut App, id: DocumentId, row_id: i64) {
    if app.doc(id).is_none() {
        return;
    }
    let pending = bind_document_row(
        app,
        id,
        DocDb::new(row_id, PublishMode::CreateOnly, rune_db::Seq(0)),
        "",
        BindMode::Restarted,
    );
    app.install_or_join_file_binding(row_id, None);
    crate::db_enqueue::replay_pending(app, id, pending);
}

pub fn adopt_scratch_doc(
    app: &mut App,
    id: DocumentId,
    row_id: i64,
    recovered: &str,
    journaled_cursors: &[rune_core::cursor::Cursor],
) {
    // Hydrated BEFORE binding: adopting after would land the recovered
    // content outside the row's lineage, and the first edit would journal
    // coordinates the row cannot replay.
    if !recovered.is_empty()
        && let Some(doc) = app.doc_mut(id)
    {
        let disk_content = doc.buffer.content().to_string();
        if let crate::document::Hydration::Refused(reason) =
            doc.hydrate(&disk_content, recovered, journaled_cursors)
        {
            messages::error(app, format!("crash recovery: {reason}"));
        }
    }
    bind_scratch_doc(app, id, row_id);
}

pub fn bind_loaded_doc(app: &mut App, id: DocumentId, doc_db: DocDb, row_content: &str) {
    let pending = bind_document_row(app, id, doc_db, row_content, BindMode::Restarted);
    crate::db_enqueue::replay_pending(app, id, pending);
}

#[cfg(test)]
#[path = "db_ack_tests.rs"]
mod tests;
