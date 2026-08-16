use rune_db::{MergeCloseState, ObsId};
use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::db::PendingOp;
use crate::document::DocumentId;
use crate::messages;

use super::resolve::{block_start, scroll_doc};
use super::session::{Block, Conflict, ConflictBlock, MergeSession, Resolution};
use super::state::MergeState;

#[derive(Clone, Serialize, Deserialize)]
struct PersistedBlock {
    start: usize,
    end: usize,
    resolved: bool,
    #[serde(default)]
    resolution: Option<Resolution>,
}

#[derive(Serialize, Deserialize)]
struct BlocksPayload {
    blocks: Vec<PersistedBlock>,
    conflicts: Vec<Conflict>,
}

fn blocks_json(pairs: &[ConflictBlock]) -> Option<String> {
    let blocks = pairs
        .iter()
        .map(|p| PersistedBlock {
            start: p.block.range.start,
            end: p.block.range.end,
            resolved: p.block.resolution.is_resolved(),
            resolution: Some(p.block.resolution),
        })
        .collect();
    let conflicts = pairs.iter().map(|p| p.conflict.clone()).collect();
    serde_json::to_string(&BlocksPayload { blocks, conflicts }).ok()
}

fn pairs_from_payload(payload: BlocksPayload) -> Vec<ConflictBlock> {
    payload
        .blocks
        .into_iter()
        .zip(payload.conflicts)
        .map(|(b, conflict)| {
            let resolution = b.resolution.unwrap_or(if b.resolved {
                Resolution::HandEdited
            } else {
                Resolution::Unresolved
            });
            ConflictBlock {
                conflict,
                block: Block {
                    range: b.start..b.end,
                    resolution,
                },
            }
        })
        .collect()
}

pub(super) fn enqueue_merge_open(
    app: &mut App,
    doc: DocumentId,
    base_obs: Option<ObsId>,
    theirs_obs: ObsId,
    marker_content: &str,
    pairs: &[ConflictBlock],
) {
    let Some(json) = blocks_json(pairs) else {
        messages::error(app, "merge state not persisted — could not be encoded");
        return;
    };
    enqueue(app, doc, |store, db_id| {
        store.merge_open(
            rune_db::DocId(db_id),
            base_obs,
            theirs_obs,
            marker_content,
            &json,
        )
    });
}

pub(super) fn enqueue_merge_progress(
    app: &mut App,
    doc: DocumentId,
    marker_content: &str,
    pairs: &[ConflictBlock],
) {
    let Some(json) = blocks_json(pairs) else {
        messages::error(app, "merge state not persisted — could not be encoded");
        return;
    };
    enqueue(app, doc, |store, db_id| {
        store.merge_progress(rune_db::DocId(db_id), marker_content, &json)
    });
}

pub(super) fn enqueue_merge_close(app: &mut App, doc: DocumentId, state: MergeCloseState) {
    enqueue(app, doc, |store, db_id| {
        store.merge_close(rune_db::DocId(db_id), state)
    });
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
        Err(e) => crate::materialize_ack::on_store_failure(app, &e.to_string()),
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
    let pairs = pairs_from_payload(payload);

    let unresolved = pairs
        .iter()
        .filter(|p| !p.block.resolution.is_resolved())
        .count();
    if unresolved == 0 {
        enqueue_merge_close(app, doc, MergeCloseState::Completed);
        return;
    }

    let marker_content = document.buffer.content().to_string();
    enqueue_merge_progress(app, doc, &marker_content, &pairs);

    let saved_display_name = super::install_resolver_display_name(app, doc);

    let blocks: Vec<Block> = pairs.iter().map(|p| p.block.clone()).collect();
    let cur = blocks
        .iter()
        .position(|b| !b.resolution.is_resolved())
        .unwrap_or(0);
    let target = block_start(&blocks, cur);
    app.merge = MergeState::Active {
        doc,
        session: MergeSession {
            conflicts: pairs,
            cur,
            saved_display_name,
            theirs_obs,
        },
    };
    scroll_doc(app, doc, target);
    messages::info(app, format!("merge resumed — {unresolved} conflict(s)"));
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    fn app_with(content: &str) -> App {
        App::new(Buffer::new(content), None, Arc::new(Mem::new()), None)
    }

    const PRE_CHANGE_BLOCKS_JSON: &str = r#"{"blocks":[{"start":5,"end":20,"resolved":false},{"start":30,"end":40,"resolved":true}],"conflicts":[{"ours":"mine","theirs":"yours"},{"ours":"a","theirs":"b"}]}"#;

    #[test]
    fn resume_decodes_the_pre_change_on_disk_shape_into_the_expected_pairing() {
        let mut app = app_with(&"x".repeat(40));
        let doc = app.active;

        resume_from_store(
            &mut app,
            doc,
            PRE_CHANGE_BLOCKS_JSON,
            ObsId::new(1).expect("nonzero"),
        );

        let MergeState::Active { session, .. } = &app.merge else {
            panic!("expected an Active merge, got {:?}", app.merge);
        };
        assert_eq!(session.cur, 0);
        assert_eq!(
            session.conflicts,
            vec![
                ConflictBlock {
                    conflict: Conflict {
                        ours: "mine".to_string(),
                        theirs: "yours".to_string(),
                    },
                    block: Block {
                        range: 5..20,
                        resolution: Resolution::Unresolved,
                    },
                },
                ConflictBlock {
                    conflict: Conflict {
                        ours: "a".to_string(),
                        theirs: "b".to_string(),
                    },
                    block: Block {
                        range: 30..40,
                        resolution: Resolution::HandEdited,
                    },
                },
            ]
        );
    }

    const POST_CHANGE_BLOCKS_JSON: &str = r#"{"blocks":[{"start":5,"end":20,"resolved":false,"resolution":"Unresolved"},{"start":30,"end":40,"resolved":true,"resolution":"TookTheirs"}],"conflicts":[{"ours":"mine","theirs":"yours"},{"ours":"a","theirs":"b"}]}"#;

    #[test]
    fn resume_decodes_the_current_on_disk_shape_and_round_trips_the_written_json_exactly() {
        let mut app = app_with(&"x".repeat(40));
        let doc = app.active;

        resume_from_store(
            &mut app,
            doc,
            POST_CHANGE_BLOCKS_JSON,
            ObsId::new(1).expect("nonzero"),
        );

        let MergeState::Active { session, .. } = &app.merge else {
            panic!("expected an Active merge, got {:?}", app.merge);
        };
        assert_eq!(session.cur, 0);
        assert_eq!(
            session.conflicts,
            vec![
                ConflictBlock {
                    conflict: Conflict {
                        ours: "mine".to_string(),
                        theirs: "yours".to_string(),
                    },
                    block: Block {
                        range: 5..20,
                        resolution: Resolution::Unresolved,
                    },
                },
                ConflictBlock {
                    conflict: Conflict {
                        ours: "a".to_string(),
                        theirs: "b".to_string(),
                    },
                    block: Block {
                        range: 30..40,
                        resolution: Resolution::TookTheirs,
                    },
                },
            ]
        );
        assert_eq!(
            blocks_json(&session.conflicts).expect("encodes"),
            POST_CHANGE_BLOCKS_JSON
        );
    }
}
