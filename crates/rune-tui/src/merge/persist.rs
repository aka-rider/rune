use rune_db::{MergeCloseState, ObsId};
use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::db::PendingOp;
use crate::document::DocumentId;
use crate::messages;

use super::session::{Block, BlockOrigin, Conflict, ConflictBlock, MergeSession, Resolution};
use super::state::MergeState;

#[derive(Clone, Serialize, Deserialize)]
struct PersistedBlock {
    start: usize,
    end: usize,
    resolved: bool,
    #[serde(default)]
    resolution: Option<Resolution>,
    #[serde(default)]
    origin: Option<BlockOrigin>,
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
            origin: Some(p.origin),
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
                origin: b.origin.unwrap_or(BlockOrigin::Conflict),
            }
        })
        .collect()
}

pub(super) fn enqueue_merge_open(
    app: &mut App,
    doc: DocumentId,
    base_obs: Option<ObsId>,
    theirs_obs: ObsId,
    content: &str,
    pairs: &[ConflictBlock],
) {
    let Some(json) = blocks_json(pairs) else {
        messages::error(app, "merge state not persisted — could not be encoded");
        return;
    };
    enqueue(app, doc, |store, db_id| {
        store.merge_open(rune_db::DocId(db_id), base_obs, theirs_obs, content, &json)
    });
}

pub(super) fn enqueue_merge_progress(
    app: &mut App,
    doc: DocumentId,
    content: &str,
    pairs: &[ConflictBlock],
) {
    let Some(json) = blocks_json(pairs) else {
        messages::error(app, "merge state not persisted — could not be encoded");
        return;
    };
    enqueue(app, doc, |store, db_id| {
        store.merge_progress(rune_db::DocId(db_id), content, &json)
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
    if let MergeState::Active {
        doc: active_doc, ..
    } = &app.merge
        && *active_doc != doc
    {
        messages::error(
            app,
            "merge not resumed — another document's merge is already active",
        );
        return;
    }
    let Ok(payload) = serde_json::from_str::<BlocksPayload>(blocks_json) else {
        messages::error(app, UNREADABLE);
        return;
    };
    let Some(document) = app.doc(doc) else { return };
    let buffer_len = document.buffer.content().len();
    let install_pos = document.journal.pos();
    if payload.blocks.len() != payload.conflicts.len()
        || !ranges_well_formed(&payload.blocks, buffer_len)
    {
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

    let content = document.buffer.content().to_string();
    let Some(theirs_text) = reconstruct_theirs(&content, &pairs) else {
        messages::error(app, UNREADABLE);
        return;
    };
    enqueue_merge_progress(app, doc, &content, &pairs);

    let saved_display_name = super::install_resolver_display_name(app, doc);

    let cur = pairs
        .iter()
        .position(|p| !p.block.resolution.is_resolved())
        .unwrap_or(0);
    let target = pairs.get(cur).map_or(0, |p| p.block.range.start);
    crate::diff_view::install_text(app, doc, theirs_text, "disk".to_string());
    app.merge = MergeState::Active {
        doc,
        session: MergeSession {
            conflicts: pairs,
            cur,
            saved_display_name,
            theirs_obs,
            install_pos,
        },
    };
    if let Some(d) = app.doc_mut(doc) {
        let clamped = target.min(d.buffer.len());
        d.cursors = rune_core::cursor::CursorSet::new(clamped);
    }
    messages::info(app, format!("merge resumed — {unresolved} conflict(s)"));
}

fn ranges_well_formed(blocks: &[PersistedBlock], buffer_len: usize) -> bool {
    let mut sorted: Vec<&PersistedBlock> = blocks.iter().collect();
    sorted.sort_by_key(|b| b.start);
    let mut prev_end = 0usize;
    for b in sorted {
        if b.start > b.end || b.end > buffer_len || b.start < prev_end {
            return false;
        }
        prev_end = b.end;
    }
    true
}

fn reconstruct_theirs(content: &str, pairs: &[ConflictBlock]) -> Option<String> {
    let mut ordered: Vec<&ConflictBlock> = pairs.iter().collect();
    ordered.sort_by_key(|p| p.block.range.start);
    let mut out = String::new();
    let mut at = 0usize;
    for pair in ordered {
        if pair.block.range.start < at {
            return None;
        }
        out.push_str(content.get(at..pair.block.range.start)?);
        out.push_str(&pair.conflict.theirs);
        at = pair.block.range.end;
    }
    out.push_str(content.get(at..)?);
    Some(out)
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
                    origin: BlockOrigin::Conflict,
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
                    origin: BlockOrigin::Conflict,
                },
            ]
        );
    }

    const BLOCKS_JSON_NO_ORIGIN_FIELD: &str = r#"{"blocks":[{"start":5,"end":20,"resolved":false,"resolution":"Unresolved"},{"start":30,"end":40,"resolved":true,"resolution":"TookTheirs"}],"conflicts":[{"ours":"mine","theirs":"yours"},{"ours":"a","theirs":"b"}]}"#;

    #[test]
    fn resume_decodes_a_resolution_bearing_row_without_origin_as_conflict_blocks() {
        let mut app = app_with(&"x".repeat(40));
        let doc = app.active;

        resume_from_store(
            &mut app,
            doc,
            BLOCKS_JSON_NO_ORIGIN_FIELD,
            ObsId::new(1).expect("nonzero"),
        );

        let MergeState::Active { session, .. } = &app.merge else {
            panic!("expected an Active merge, got {:?}", app.merge);
        };
        assert_eq!(session.cur, 0);
        assert!(
            session
                .conflicts
                .iter()
                .all(|p| p.origin == BlockOrigin::Conflict)
        );
        assert_eq!(
            session.conflicts.first().map(|p| p.block.resolution),
            Some(Resolution::Unresolved)
        );
        assert_eq!(
            session.conflicts.get(1).map(|p| p.block.resolution),
            Some(Resolution::TookTheirs)
        );
    }

    const POST_CHANGE_BLOCKS_JSON: &str = r#"{"blocks":[{"start":5,"end":20,"resolved":false,"resolution":"Unresolved","origin":"Conflict"},{"start":30,"end":40,"resolved":true,"resolution":"TookTheirs","origin":"AutoApplied"}],"conflicts":[{"ours":"mine","theirs":"yours"},{"ours":"a","theirs":"b"}]}"#;

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
                    origin: BlockOrigin::Conflict,
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
                    origin: BlockOrigin::AutoApplied,
                },
            ]
        );
        assert_eq!(
            blocks_json(&session.conflicts).expect("encodes"),
            POST_CHANGE_BLOCKS_JSON
        );
    }

    #[test]
    fn resume_rebuilds_the_left_pane_from_the_buffer_and_stored_theirs() {
        let mut app = app_with(&"x".repeat(40));
        let doc = app.active;

        resume_from_store(
            &mut app,
            doc,
            BLOCKS_JSON_NO_ORIGIN_FIELD,
            ObsId::new(1).expect("nonzero"),
        );

        let diff = app
            .diff
            .as_ref()
            .expect("resume must install the pane view");
        assert_eq!(diff.right, doc);
        let content = "x".repeat(40);
        let expected = format!("{}yours{}b", &content[..5], &content[20..30]);
        assert_eq!(diff.left.buffer.content(), expected);
    }

    #[test]
    fn resume_refuses_to_clobber_an_active_merge_on_another_document() {
        let mut app = app_with(&"x".repeat(40));
        let doc = app.active;
        let other = app.open_document(Buffer::new("other"));
        let other_state = MergeState::Active {
            doc: other,
            session: MergeSession {
                conflicts: Vec::new(),
                cur: 0,
                saved_display_name: Some("other-name".to_string()),
                theirs_obs: ObsId::new(9).expect("nonzero"),
                install_pos: 0,
            },
        };
        app.merge = other_state.clone();

        resume_from_store(
            &mut app,
            doc,
            BLOCKS_JSON_NO_ORIGIN_FIELD,
            ObsId::new(1).expect("nonzero"),
        );

        assert_eq!(app.merge, other_state);
        assert!(app.diff.is_none());
        assert_eq!(
            messages::newest_text(&app),
            Some("merge not resumed — another document's merge is already active")
        );
    }

    #[test]
    fn resume_rejects_overlapping_ranges_as_malformed() {
        let mut app = app_with(&"x".repeat(40));
        let doc = app.active;
        let overlapping = r#"{"blocks":[{"start":5,"end":20,"resolved":false},{"start":10,"end":15,"resolved":false}],"conflicts":[{"ours":"mine","theirs":"yours"},{"ours":"a","theirs":"b"}]}"#;

        resume_from_store(&mut app, doc, overlapping, ObsId::new(1).expect("nonzero"));

        assert_eq!(app.merge, MergeState::Inactive);
        assert!(app.diff.is_none());
        assert_eq!(
            messages::newest_text(&app),
            Some("merge not resumed — stored merge state could not be read")
        );
    }

    #[test]
    fn resume_rejects_duplicate_ranges_as_malformed() {
        let mut app = app_with(&"x".repeat(40));
        let doc = app.active;
        let duplicate = r#"{"blocks":[{"start":5,"end":20,"resolved":false},{"start":5,"end":20,"resolved":false}],"conflicts":[{"ours":"mine","theirs":"yours"},{"ours":"a","theirs":"b"}]}"#;

        resume_from_store(&mut app, doc, duplicate, ObsId::new(1).expect("nonzero"));

        assert_eq!(app.merge, MergeState::Inactive);
        assert!(app.diff.is_none());
    }
}
