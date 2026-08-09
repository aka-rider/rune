use rune_db::{MergeCloseState, ObsId};
use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::commands::nav_scroll;
use crate::db::PendingOp;
use crate::document::DocumentId;
use crate::messages;

use super::state::{Block, Conflict, MergeState};

#[derive(Serialize, Deserialize)]
struct BlocksPayload {
    blocks: Vec<Block>,
    conflicts: Vec<Conflict>,
}

fn blocks_json(blocks: &[Block], conflicts: &[Conflict]) -> Option<String> {
    serde_json::to_string(&BlocksPayload {
        blocks: blocks.to_vec(),
        conflicts: conflicts.to_vec(),
    })
    .ok()
}

pub(super) fn enqueue_merge_open(
    app: &mut App,
    doc: DocumentId,
    base_obs: Option<ObsId>,
    theirs_obs: ObsId,
    marker_content: &str,
    blocks: &[Block],
    conflicts: &[Conflict],
) {
    let Some(json) = blocks_json(blocks, conflicts) else {
        messages::error(app, "merge state not persisted — could not be encoded");
        return;
    };
    enqueue(app, doc, |store, db_id| {
        store.merge_open(db_id, base_obs, theirs_obs, marker_content, &json)
    });
}

pub(super) fn enqueue_merge_progress(
    app: &mut App,
    doc: DocumentId,
    marker_content: &str,
    blocks: &[Block],
    conflicts: &[Conflict],
) {
    let Some(json) = blocks_json(blocks, conflicts) else {
        messages::error(app, "merge state not persisted — could not be encoded");
        return;
    };
    enqueue(app, doc, |store, db_id| {
        store.merge_progress(db_id, marker_content, &json)
    });
}

pub(super) fn enqueue_merge_close(app: &mut App, doc: DocumentId, state: MergeCloseState) {
    enqueue(app, doc, |store, db_id| store.merge_close(db_id, state));
}

fn enqueue(
    app: &mut App,
    doc: DocumentId,
    submit: impl FnOnce(&rune_db::Store, i64) -> Result<u64, rune_db::Error>,
) {
    let Some(db_id) = app.doc_db_id(doc) else {
        return;
    };
    let Some(db) = app.db.as_ref() else { return };
    if db.degraded {
        return;
    }
    match submit(&db.store, db_id) {
        Ok(op_id) => {
            app.db_ops.insert(op_id, PendingOp::new(doc));
        }
        Err(e) => crate::materialize_ack::on_store_failure(app, e.to_string()),
    }
}

pub(crate) fn resume_from_store(
    app: &mut App,
    doc: DocumentId,
    blocks_json: &str,
    theirs_obs: ObsId,
) {
    const UNREADABLE: &str = "merge not resumed — stored merge state could not be read";
    let Ok(payload) = serde_json::from_str::<BlocksPayload>(blocks_json) else {
        messages::error(app, UNREADABLE);
        return;
    };
    let Some(document) = app.doc(doc) else { return };
    let buffer_len = document.buffer.content().len();
    let well_formed = payload.blocks.len() == payload.conflicts.len()
        && payload
            .blocks
            .iter()
            .all(|b| b.start <= b.end && b.end <= buffer_len);
    if !well_formed {
        messages::error(app, UNREADABLE);
        return;
    }
    let unresolved = payload.blocks.iter().filter(|b| !b.resolved).count();
    if unresolved == 0 {
        enqueue_merge_close(app, doc, MergeCloseState::Completed);
        return;
    }

    let marker_content = document.buffer.content().to_string();
    enqueue_merge_progress(
        app,
        doc,
        &marker_content,
        &payload.blocks,
        &payload.conflicts,
    );

    let file_name = app
        .doc(doc)
        .map(|d| d.file_name().to_string())
        .unwrap_or_default();
    let saved_display_name = app.doc(doc).and_then(|d| d.display_name.clone());
    if let Some(d) = app.doc_mut(doc) {
        d.display_name = Some(format!("{file_name}: editor <-> disk"));
    }

    let cur = payload.blocks.iter().position(|b| !b.resolved).unwrap_or(0);
    let target = payload.blocks.get(cur).map(|b| b.start).unwrap_or(0);
    app.merge = MergeState::Active {
        doc,
        conflicts: payload.conflicts,
        blocks: payload.blocks,
        cur,
        saved_display_name,
        theirs_obs,
    };
    if let Some(d) = app.doc_mut(doc) {
        nav_scroll::scroll_to_byte_offset(d, target);
    }
    messages::info(app, format!("merge resumed — {unresolved} conflict(s)"));
}
