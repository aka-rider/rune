mod landing;
mod persist;
pub(crate) mod ranges;
pub mod session;
pub mod state;
pub(crate) mod verbs;

pub(crate) use landing::handle_merge_prep_ack;
pub(crate) use persist::resume_from_store;
pub use session::{Block, BlockOrigin, Conflict, ConflictBlock, MergeSession, Resolution};
pub use state::{MergeIntent, MergeState};

pub(crate) const VERB_HINT: &str = "⇧⌘Y take disk · ⇧⌘U keep yours · ⇧⌘J/^K next/prev · Esc close";
pub(crate) const NO_DIVERGENCE_REASON: &str = "no divergence to merge";

use rune_db::SyncKind;

use crate::app::App;
use crate::db::PendingOp;
use crate::document::Document;
use crate::messages;
use crate::runtime::Effects;

pub(crate) fn is_divergent(doc: &Document) -> bool {
    doc.last_sync.is_some_and(SyncKind::is_disk_divergent)
}

pub(crate) fn begin(app: &mut App, intent: MergeIntent, _effects: &mut Effects) {
    let id = app.active;
    if matches!(&app.merge, MergeState::Pending { doc: d, .. } if *d == id) {
        messages::warn(app, "merge already preparing");
        return;
    }
    if matches!(app.merge, MergeState::Active { .. }) {
        abandon_active_before_fresh_begin(app);
    }
    let Some(doc) = app.doc(id) else { return };
    if doc.save_in_flight() {
        messages::warn(app, "save in progress — merge after it completes");
        return;
    }
    if !is_divergent(doc) {
        messages::warn(app, NO_DIVERGENCE_REASON);
        return;
    }
    let Some(db_id) = doc.doc_db().map(|d| d.db_id) else {
        messages::warn(app, NO_DIVERGENCE_REASON);
        return;
    };
    let Some(db) = app.db.as_ref() else {
        messages::warn(app, NO_DIVERGENCE_REASON);
        return;
    };
    if db.degraded {
        messages::warn(app, NO_DIVERGENCE_REASON);
        return;
    }

    match db.store.merge_prep(rune_db::DocId(db_id)) {
        Ok(op_id) => {
            let generation = app.next_merge_gen.mint();
            app.merge = MergeState::Pending {
                doc: id,
                generation,
                intent,
            };
            app.db_ops
                .insert(op_id, PendingOp::merge_prep(id, generation));
        }
        Err(e) => crate::materialize_ack::on_store_failure(app, &e.to_string()),
    }
}

fn abandon_active_before_fresh_begin(app: &mut App) {
    let MergeState::Active { doc, session } = std::mem::take(&mut app.merge) else {
        return;
    };
    let unresolved = session.unresolved_count();
    let message = if unresolved == 0 {
        "merge closed — starting a fresh merge against newer disk changes".to_string()
    } else {
        format!("merge closed — {unresolved} unresolved conflict(s) left behind for a fresh merge")
    };
    abandon_active(app, doc, session.saved_display_name, message);
}

pub(crate) fn exit_in_place(app: &mut App) {
    if !matches!(app.merge, MergeState::Active { .. }) {
        return;
    }
    let MergeState::Active { doc, session } = std::mem::take(&mut app.merge) else {
        return;
    };
    crate::diff_view::teardown(app, doc);
    let unresolved = session.unresolved_count();
    if unresolved == 0 {
        if let Some(d) = app.doc_mut(doc) {
            d.display_name = session.saved_display_name;
        }
        set_last_sync(app, doc, SyncKind::BufferAhead);
        landing::advance_expect_obs(app, doc, session.theirs_obs);
        persist::enqueue_merge_close(app, doc, rune_db::MergeCloseState::Completed);
        let save_key = crate::global::label_for(crate::global::GlobalCommand::Save);
        messages::info(app, format!("merge complete — {save_key} to save"));
    } else {
        abandon_active(
            app,
            doc,
            session.saved_display_name,
            format!("merge closed — {unresolved} unresolved conflict(s) remain"),
        );
    }
}

fn abandon_active(
    app: &mut App,
    doc: crate::document::DocumentId,
    saved_display_name: Option<String>,
    message: impl Into<String>,
) {
    crate::diff_view::teardown(app, doc);
    if let Some(d) = app.doc_mut(doc) {
        d.display_name = saved_display_name;
    }
    enqueue_resolve_abandon(app, doc);
    persist::enqueue_merge_close(app, doc, rune_db::MergeCloseState::Abandoned);
    messages::info(app, message);
}

pub(crate) fn retract_active_on_convergence(
    app: &mut App,
    doc: crate::document::DocumentId,
    kind: SyncKind,
) {
    if kind.is_disk_divergent() {
        return;
    }
    let nothing_resolved_yet = matches!(
        &app.merge,
        MergeState::Active { doc: d, session }
            if *d == doc && session.conflicts.iter().all(|c| match c.block.origin {
                session::BlockOrigin::Conflict => {
                    c.block.resolution == session::Resolution::Unresolved
                }
                session::BlockOrigin::AutoApplied => {
                    c.block.resolution == session::Resolution::TookTheirs
                }
            })
    );
    if !nothing_resolved_yet {
        return;
    }
    let MergeState::Active { session, .. } = std::mem::take(&mut app.merge) else {
        return;
    };
    abandon_active(
        app,
        doc,
        session.saved_display_name,
        "disk settled — nothing left to merge",
    );
}

fn install_resolver_display_name(
    app: &mut App,
    doc: crate::document::DocumentId,
) -> Option<String> {
    let file_name = app
        .doc(doc)
        .map(|d| d.file_name().to_string())
        .unwrap_or_default();
    let saved_display_name = app.doc(doc).and_then(|d| d.display_name.clone());
    if let Some(d) = app.doc_mut(doc) {
        d.display_name = Some(format!("{file_name}: editor <-> disk"));
    }
    saved_display_name
}

fn set_last_sync(app: &mut App, doc: crate::document::DocumentId, kind: SyncKind) {
    if let Some(d) = app.doc_mut(doc) {
        d.last_sync = Some(kind);
    }
}

fn enqueue_resolve_abandon(app: &mut App, doc: crate::document::DocumentId) {
    let Some(db_id) = app.doc_db_id(doc) else {
        return;
    };
    let Some(db) = app.db.as_ref() else { return };
    if db.degraded {
        return;
    }
    match db.store.resolve_abandon(rune_db::DocId(db_id)) {
        Ok(op_id) => {
            app.db_ops.insert(op_id, PendingOp::new(doc));
            if let Some(binding) = app.doc_file_binding_mut(doc) {
                binding.baseline_epoch = binding.baseline_epoch.wrapping_add(1);
            }
        }
        Err(e) => crate::materialize_ack::on_store_failure(app, &e.to_string()),
    }
}

pub(crate) fn cancel_pending(app: &mut App) {
    if !matches!(app.merge, MergeState::Pending { .. }) {
        return;
    }
    app.merge = MergeState::Inactive;
    messages::warn(
        app,
        "merge cancelled — the document changed before disk state arrived",
    );
}

pub(crate) fn auto_exit(app: &mut App) {
    match &app.merge {
        MergeState::Active { .. } => exit_in_place(app),
        MergeState::Pending { .. } => cancel_pending(app),
        MergeState::Inactive => {}
    }
}

pub(crate) fn refuses_save(app: &mut App, target: crate::document::DocumentId) -> bool {
    if matches!(&app.merge, MergeState::Pending { doc, .. } if *doc == target) {
        let save_key = crate::global::label_for(crate::global::GlobalCommand::Save);
        messages::warn(
            app,
            format!("merge is preparing — press {save_key} again once it lands"),
        );
        return true;
    }
    let MergeState::Active { doc, .. } = &app.merge else {
        return false;
    };
    if *doc != target {
        return false;
    }
    let unresolved = app.merge.unresolved_count();
    if unresolved == 0 {
        return false;
    }
    messages::warn(
        app,
        format!("{unresolved} conflict(s) to resolve — {VERB_HINT}"),
    );
    true
}

pub(crate) fn is_active_on(app: &App, target: crate::document::DocumentId) -> bool {
    matches!(app.merge, MergeState::Active { doc, .. } if doc == target)
}

pub(crate) fn toggle(app: &mut App, effects: &mut Effects) {
    if matches!(app.merge, MergeState::Active { .. }) {
        exit_in_place(app);
    } else {
        begin(app, MergeIntent::Merge, effects);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    fn app_with(content: &str) -> App {
        App::new(Buffer::new(content), None, Arc::new(Mem::new()), None)
    }

    fn active_with_blocks(doc: crate::document::DocumentId, blocks: Vec<Block>) -> MergeState {
        MergeState::Active {
            doc,
            session: MergeSession {
                conflicts: blocks
                    .into_iter()
                    .map(|block| ConflictBlock {
                        conflict: Conflict {
                            ours: "ours".to_string(),
                            theirs: "theirs".to_string(),
                        },
                        block,
                    })
                    .collect(),
                cur: 0,
                saved_display_name: Some("saved-name".to_string()),
                theirs_obs: rune_db::ObsId::new(7).expect("nonzero"),
                install_pos: 0,
            },
        }
    }

    #[test]
    fn retract_active_on_convergence_is_a_noop_while_still_divergent() {
        let mut app = app_with("hello");
        let doc = app.active;
        app.merge = active_with_blocks(
            doc,
            vec![Block {
                range: 0..5,
                resolution: Resolution::Unresolved,
                origin: BlockOrigin::Conflict,
            }],
        );

        retract_active_on_convergence(&mut app, doc, SyncKind::Diverged);

        assert!(matches!(app.merge, MergeState::Active { .. }));
    }

    #[test]
    fn retract_active_on_convergence_is_a_noop_once_anything_is_resolved() {
        let mut app = app_with("hello");
        let doc = app.active;
        app.merge = active_with_blocks(
            doc,
            vec![
                Block {
                    range: 0..5,
                    resolution: Resolution::TookTheirs,
                    origin: BlockOrigin::Conflict,
                },
                Block {
                    range: 5..9,
                    resolution: Resolution::Unresolved,
                    origin: BlockOrigin::Conflict,
                },
            ],
        );

        retract_active_on_convergence(&mut app, doc, SyncKind::Clean);

        assert!(matches!(app.merge, MergeState::Active { .. }));
    }

    #[test]
    fn retract_active_on_convergence_exits_cleanly_with_nothing_resolved() {
        let mut app = app_with("hello");
        let doc = app.active;
        if let Some(d) = app.doc_mut(doc) {
            d.display_name = Some("original".to_string());
        }
        app.merge = active_with_blocks(
            doc,
            vec![Block {
                range: 0..5,
                resolution: Resolution::Unresolved,
                origin: BlockOrigin::Conflict,
            }],
        );

        retract_active_on_convergence(&mut app, doc, SyncKind::BufferAhead);

        assert_eq!(app.merge, MergeState::Inactive);
        assert_eq!(
            app.doc(doc).unwrap().display_name,
            Some("saved-name".to_string())
        );
        assert_eq!(
            messages::newest_text(&app),
            Some("disk settled — nothing left to merge")
        );
    }

    #[test]
    fn retract_active_on_convergence_ignores_a_different_document() {
        let mut app = app_with("hello");
        let doc = app.active;
        app.merge = active_with_blocks(
            doc,
            vec![Block {
                range: 0..5,
                resolution: Resolution::Unresolved,
                origin: BlockOrigin::Conflict,
            }],
        );
        let other = app.open_document(Buffer::new("other"));

        retract_active_on_convergence(&mut app, other, SyncKind::Clean);

        assert!(matches!(app.merge, MergeState::Active { .. }));
    }
}
