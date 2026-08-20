use rune_core::buffer::Edit;
use rune_core::cursor::{CursorId, CursorSet};
use rune_core::undo::EditKind;
#[cfg(test)]
use rune_db::BlobHash;
use rune_db::{MergePrepOutcome, MergePrepResult, ObsId, SyncKind};

use crate::app::App;
use crate::commands::edit_core::apply_edit_batch_with_cursors;
use crate::db::PendingOp;
use crate::document::DocumentId;
use crate::messages;
use crate::runtime::Effects;

use super::session::{Block, BlockOrigin, Conflict, ConflictBlock, MergeSession, Resolution};
use super::state::MergeIntent;
use super::state::MergeState;

const UTF8_REFUSAL: &str = "merge unavailable — the file on disk is not valid UTF-8";

pub(crate) fn handle_merge_prep_ack(
    app: &mut App,
    doc: DocumentId,
    merge_gen: Option<crate::generation::MergeGen>,
    prep: MergePrepResult,
    _effects: &mut Effects,
) {
    let ticket = match (&app.merge, merge_gen) {
        (
            MergeState::Pending {
                doc: d,
                generation,
                intent,
            },
            Some(g),
        ) if *d == doc && *generation == g => Some(*intent),
        _ => None,
    };
    let Some(intent) = ticket else {
        return;
    };

    if app
        .doc(doc)
        .is_some_and(crate::document::Document::save_in_flight)
    {
        super::set_last_sync(app, doc, prep.sync.kind);
        app.merge = MergeState::Inactive;
        let merge_key = crate::global::label_for(crate::global::GlobalCommand::Merge);
        messages::warn(
            app,
            format!("save in flight — merge cancelled, press {merge_key} once it completes"),
        );
        return;
    }

    if !prep.sync.kind.is_disk_divergent() {
        super::set_last_sync(app, doc, prep.sync.kind);
        app.merge = MergeState::Inactive;
        messages::info(app, "file on disk matches — nothing to merge");
        return;
    }

    let MergePrepOutcome::Ready { ancestor, theirs } = prep.outcome else {
        app.merge = MergeState::Inactive;
        messages::error(app, "disk is changing — try again");
        return;
    };

    let Some((theirs_obs, theirs_bytes)) = theirs else {
        app.merge = MergeState::Inactive;
        messages::error(app, "merge unavailable — no disk version to merge against");
        return;
    };

    let Ok(theirs_text) = String::from_utf8(theirs_bytes) else {
        app.merge = MergeState::Inactive;
        messages::error(app, UTF8_REFUSAL);
        return;
    };

    if intent == MergeIntent::Discard {
        discard_install(app, doc, &theirs_text, theirs_obs);
        return;
    }

    let ancestor_text = match ancestor {
        None => None,
        Some((_rung, bytes)) => match String::from_utf8(bytes) {
            Ok(text) => Some(text),
            Err(_) => {
                app.merge = MergeState::Inactive;
                messages::error(app, UTF8_REFUSAL);
                return;
            }
        },
    };

    let Some(active) = app.doc(doc) else {
        app.merge = MergeState::Inactive;
        return;
    };
    let ours_text = active.buffer.content().to_string();

    let hunks = match &ancestor_text {
        Some(text) => rune_merge::merge_hunks(
            text.as_bytes(),
            ours_text.as_bytes(),
            theirs_text.as_bytes(),
        ),
        None => {
            messages::info(
                app,
                "no saved ancestor for this file — showing all differences as conflicts",
            );
            rune_merge::merge_hunks_no_ancestor(ours_text.as_bytes(), theirs_text.as_bytes())
        }
    };
    let Ok((buffer_text, pane_theirs_text, mut pairs)) = build_pane_install(&hunks) else {
        app.merge = MergeState::Inactive;
        messages::error(app, UTF8_REFUSAL);
        return;
    };

    let first_start = pairs.first().map_or(0, |p| p.block.range.start);
    if !install_whole_range(app, doc, &buffer_text, first_start) {
        app.merge = MergeState::Inactive;
        messages::error(app, "merge failed — the document could not be updated");
        return;
    }
    let install_pos = app.doc(doc).map_or(0, |d| d.journal.pos());
    let adopted = enqueue_resolve_adopt(app, doc, theirs_obs);

    if pairs.is_empty() {
        if adopted {
            advance_expect_obs(app, doc, theirs_obs);
        }
        let installed_matches_theirs = app
            .doc(doc)
            .is_some_and(|d| d.buffer.content() == theirs_text);
        super::set_last_sync(
            app,
            doc,
            if installed_matches_theirs {
                SyncKind::Clean
            } else {
                SyncKind::BufferAhead
            },
        );
        app.merge = MergeState::Inactive;
        messages::info(app, "merged cleanly — disk changes applied");
        return;
    }

    let saved_display_name = super::install_resolver_display_name(app, doc);

    let unresolved = pairs.len();
    pairs.extend(auto_applied_entries(&ours_text, &buffer_text, &pairs));
    pairs.sort_by_key(|p| p.block.range.start);
    let cur = pairs
        .iter()
        .position(|p| !p.block.resolution.is_resolved())
        .unwrap_or(0);
    super::persist::enqueue_merge_open(
        app,
        doc,
        prep.sync.ancestor.as_ref().and_then(|v| v.obs),
        theirs_obs,
        &buffer_text,
        &pairs,
    );
    crate::diff_view::install_text(app, doc, pane_theirs_text, "disk".to_string());
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
    messages::info(
        app,
        format!("{unresolved} conflict(s) to resolve — {}", super::VERB_HINT),
    );
}

struct MergeUtf8Error;

fn build_pane_install(
    hunks: &[rune_merge::Hunk],
) -> Result<(String, String, Vec<ConflictBlock>), MergeUtf8Error> {
    let mut merged = String::new();
    let mut theirs_doc = String::new();
    let mut pairs = Vec::new();
    for hunk in hunks {
        match hunk {
            rune_merge::Hunk::Clean(bytes) => {
                let text = std::str::from_utf8(bytes).map_err(|_| MergeUtf8Error)?;
                merged.push_str(text);
                theirs_doc.push_str(text);
            }
            rune_merge::Hunk::Conflict { ours, theirs } => {
                let ours = std::str::from_utf8(ours).map_err(|_| MergeUtf8Error)?;
                let theirs = std::str::from_utf8(theirs).map_err(|_| MergeUtf8Error)?;
                let start = merged.len();
                merged.push_str(ours);
                theirs_doc.push_str(theirs);
                pairs.push(ConflictBlock {
                    conflict: Conflict {
                        ours: ours.to_string(),
                        theirs: theirs.to_string(),
                    },
                    block: Block {
                        range: start..merged.len(),
                        resolution: Resolution::Unresolved,
                        origin: BlockOrigin::Conflict,
                    },
                });
            }
        }
    }
    Ok((merged, theirs_doc, pairs))
}

fn auto_applied_entries(
    pre_merge: &str,
    merged: &str,
    conflicts: &[ConflictBlock],
) -> Vec<ConflictBlock> {
    use rune_merge::RegionKind;

    let map = rune_merge::align(pre_merge, merged);
    let mut entries = Vec::new();
    for region in &map.regions {
        if !matches!(region.kind, RegionKind::Changed | RegionKind::RightOnly) {
            continue;
        }
        let range = crate::diff_view::rows::line_byte_range(merged, region.right_lines.clone());
        let clashes = conflicts
            .iter()
            .any(|c| c.block.range.start < range.end && range.start < c.block.range.end);
        if clashes {
            continue;
        }
        let ours_range =
            crate::diff_view::rows::line_byte_range(pre_merge, region.left_lines.clone());
        let ours = pre_merge.get(ours_range).unwrap_or_default().to_string();
        let theirs = merged.get(range.clone()).unwrap_or_default().to_string();
        entries.push(ConflictBlock {
            conflict: Conflict { ours, theirs },
            block: Block {
                range,
                resolution: Resolution::TookTheirs,
                origin: BlockOrigin::AutoApplied,
            },
        });
    }
    entries
}

fn discard_install(app: &mut App, doc: DocumentId, theirs_text: &str, theirs_obs: ObsId) {
    if !install_whole_range(app, doc, theirs_text, 0) {
        app.merge = MergeState::Inactive;
        messages::error(app, "merge failed — the document could not be updated");
        return;
    }
    if enqueue_resolve_adopt(app, doc, theirs_obs) {
        advance_expect_obs(app, doc, theirs_obs);
    }
    super::set_last_sync(app, doc, SyncKind::Clean);
    app.merge = MergeState::Inactive;
    messages::info(app, "disk changes adopted");
}

fn install_whole_range(app: &mut App, doc: DocumentId, text: &str, cursor_at: usize) -> bool {
    let Some(document) = app.doc(doc) else {
        return false;
    };
    let cursors_before = document.cursors.clone();
    let old_len = document.buffer.content().len();
    let edit = Edit {
        start: 0,
        end: old_len,
        insert: text.to_string(),
    };
    apply_edit_batch_with_cursors(
        app,
        doc,
        vec![(edit, CursorId::FIRST)],
        &cursors_before,
        EditKind::Other,
        move |_, _| vec![CursorSet::new(cursor_at).primary()],
    )
}

fn enqueue_resolve_adopt(app: &mut App, doc: DocumentId, theirs_obs: ObsId) -> bool {
    let Some(db_id) = app.doc_db_id(doc) else {
        return false;
    };
    let Some(db) = app.db.as_ref() else {
        return false;
    };
    if db.degraded {
        return false;
    }
    match db
        .store
        .resolve_adopt(rune_db::DocId(db_id), theirs_obs, None)
    {
        Ok(op_id) => {
            app.db_ops.insert(op_id, PendingOp::new(doc));
            true
        }
        Err(e) => {
            crate::materialize_ack::on_store_failure(app, &e.to_string());
            false
        }
    }
}

pub(super) fn advance_expect_obs(app: &mut App, doc: DocumentId, theirs_obs: ObsId) {
    if let Some(binding) = app.doc_file_binding_mut(doc) {
        binding.expect_obs = Some(theirs_obs);
        binding.baseline_epoch = binding.baseline_epoch.wrapping_add(1);
    }
}

#[cfg(test)]
#[path = "landing_tests.rs"]
mod tests;
