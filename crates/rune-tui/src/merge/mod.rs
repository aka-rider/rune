//! Merge mode (plan "merge-user-s-changes-with-idempotent-octopus", WP3):
//! entry (`begin`), the `MergePrep` ack landing (`landing`), and the
//! Esc-exit-in-place chokepoint (`exit_in_place`). The resolver's own keys/
//! navigation/accept (WP4), painting (WP5), and resync/auto-exit/guard
//! (WP6) are later work packages layered on the state this module owns.

pub mod frame;
pub mod keys;
mod landing;
pub(crate) mod paint;
mod persist;
mod resolve;
pub(crate) mod resync;
pub mod state;

pub use keys::{MERGE_BINDINGS, MergeCommand};
pub(crate) use landing::handle_merge_prep_ack;
pub(crate) use persist::resume_from_store;
pub(crate) use resync::resync;
pub use state::{Block, Conflict, MergeIntent, MergeState};

use rune_db::SyncKind;

use crate::app::App;
use crate::db::PendingOp;
use crate::messages;
use crate::runtime::Effects;

/// `^M` (plan WP3.S5): a fast pre-check against `Document.last_sync`
/// (render/hint state only, plan Gotchas `[R3]`) before ever enqueueing a
/// `MergePrep` — refuses immediately, with feedback, when there is
/// obviously nothing to merge, without waiting on a round trip to confirm
/// what the hint already rules out. The fresh `MergePrep` landing
/// (`landing::handle_merge_prep_ack`) re-checks authoritatively before
/// actually installing anything.
pub(crate) fn begin(app: &mut App, intent: MergeIntent, _effects: &mut Effects) {
    let id = app.active;
    let Some(doc) = app.doc(id) else { return };
    // A save's multi-hop materialize dance is still in flight for this
    // document: a `MergePrep` enqueued now could land AFTER the save's
    // commit ack and rebase `DocDb::expect_obs` backwards to the pre-save
    // disk observation, making the very next ⌘S CAS-refuse against a file
    // the session itself just wrote. `save_in_flight` spans the whole dance
    // — trigger through the same ack that advances `expect_obs` — unlike
    // any single pending-op entry, so it is the one check with no window.
    if doc.save_in_flight() {
        messages::warn(app, "save in progress — merge after it completes");
        return;
    }
    if !doc.last_sync.is_some_and(SyncKind::is_disk_divergent) {
        messages::warn(app, "no divergence to merge");
        return;
    }
    let Some(db_id) = doc.doc_db().map(|d| d.db_id) else {
        messages::warn(app, "no divergence to merge");
        return;
    };
    let Some(db) = app.db.as_ref() else {
        messages::warn(app, "no divergence to merge");
        return;
    };
    if db.degraded {
        messages::warn(app, "no divergence to merge");
        return;
    }

    match db.store.merge_prep(rune_db::DocId(db_id)) {
        Ok(op_id) => {
            let generation = app.next_merge_gen;
            app.next_merge_gen = app.next_merge_gen.wrapping_add(1);
            app.merge = MergeState::Pending {
                doc: id,
                generation,
                intent,
            };
            app.db_ops
                .insert(op_id, PendingOp::merge_prep(id, generation));
        }
        Err(e) => crate::materialize_ack::on_store_failure(app, e.to_string()),
    }
}

/// The exit-in-place chokepoint (plan WP3.S7, decision 4: "Esc = exit in
/// place — no abort-restore"). A no-op outside `Active` — checked BEFORE
/// ever taking `app.merge` (review fix F3): a `Pending` attempt has no
/// `saved_display_name`/blocks to restore or count, and taking it
/// unconditionally used to silently cancel it with no feedback at all,
/// contradicting this very doc's "no-op outside `Active`" — see
/// `cancel_pending` below for `Pending`'s own exit path, with its own
/// status. `pub(crate)`'s only caller in THIS work package is `^M` toggling
/// off an already-`Active` merge by hand; later work packages (auto-exit on
/// tab switch/close/quit, the resolver's own Esc, fully-resolved auto-exit)
/// add more callers without changing this function's contract.
pub(crate) fn exit_in_place(app: &mut App) {
    if !matches!(app.merge, MergeState::Active { .. }) {
        return;
    }
    let MergeState::Active {
        doc,
        blocks,
        saved_display_name,
        theirs_obs,
        ..
    } = std::mem::take(&mut app.merge)
    else {
        return;
    };
    let unresolved = blocks.iter().filter(|b| !b.resolved).count();
    if unresolved == 0 {
        if let Some(d) = app.doc_mut(doc) {
            d.display_name = saved_display_name;
        }
        // A completed merge leaves the buffer strictly ahead
        // of the disk bytes it just reconciled with — recording that
        // here is what retires the disk-changed banner/hint instead of
        // re-inviting a merge forever. Esc-out with unresolved blocks
        // leaves `last_sync` untouched: the document is still
        // truthfully diverged and the affordances must return.
        set_last_sync(app, doc, SyncKind::BufferAhead);
        // Completion is the terminal success: only NOW has the user
        // genuinely reconciled the buffer with the disk bytes the merge
        // read, so only now does the save-CAS baseline advance to them.
        landing::advance_expect_obs(app, doc, theirs_obs);
        persist::enqueue_merge_close(app, doc, rune_db::MergeRowState::Completed);
        messages::info(app, "merge complete — \u{2318}S to save");
    } else {
        // An unresolved retirement (Esc, ^M toggle-off, a tab switch/close/
        // quit auto-exit) retracts the entry-time resolve observation:
        // without this, the resolve row at the journal head makes the very
        // next probe classify the marker-filled buffer as reconciled —
        // retiring every divergence affordance and making `begin` refuse
        // with "no divergence to merge" while the conflict is anything but
        // resolved. Abandoning restores the pre-merge baseline so the
        // document classifies `Diverged` again and `^M` re-enters a real
        // merge. The save-CAS baseline was never advanced on this path,
        // so a ⌘S still CAS-refuses into the disk-conflict guard.
        abandon_active(
            app,
            doc,
            saved_display_name,
            format!("merge closed — {unresolved} unresolved marker block(s) remain"),
        );
    }
}

/// The shared unresolved-retirement path: restores the document's display
/// name, retracts the entry-time resolve observation, and posts `message`.
/// Used by [`exit_in_place`] (Esc, `^M` toggle-off, an unresolved auto-exit)
/// and by [`retract_active_on_convergence`] (nothing resolved yet, and a
/// later probe says the divergence that prompted the merge is gone).
fn abandon_active(
    app: &mut App,
    doc: crate::document::DocumentId,
    saved_display_name: Option<String>,
    message: impl Into<String>,
) {
    if let Some(d) = app.doc_mut(doc) {
        d.display_name = saved_display_name;
    }
    enqueue_resolve_abandon(app, doc);
    persist::enqueue_merge_close(app, doc, rune_db::MergeRowState::Abandoned);
    messages::info(app, message);
}

/// Extends the entry-time "file on disk matches — nothing to merge"
/// check (`landing::handle_merge_prep_ack`) to an already-`Active` merge:
/// when nothing has been resolved yet and a later probe finds disk no
/// longer diverged from this session's own reconstruction, whatever
/// prompted the merge is gone — a clean exit beats leaving a stale
/// conflict UI up, and there is no resolver progress to lose. A no-op
/// once any block has been resolved, or for any document other than the
/// one `Active` names.
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
        MergeState::Active { doc: d, blocks, .. }
            if *d == doc && blocks.iter().all(|b| !b.resolved)
    );
    if !nothing_resolved_yet {
        return;
    }
    let MergeState::Active {
        saved_display_name, ..
    } = std::mem::take(&mut app.merge)
    else {
        return;
    };
    abandon_active(
        app,
        doc,
        saved_display_name,
        "disk settled — nothing left to merge",
    );
}

/// Installs the resolver's "name: editor <-> disk" display title on `doc`
/// and returns the display name it replaced, for `MergeState::Active` to
/// carry until exit restores it.
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

/// The one place merge outcomes record a document's fresh sync
/// classification — render/hint state only, never the CAS baseline (that is
/// `landing::advance_expect_obs`'s job, gated on terminal success). A no-op
/// for a document that vanished mid-ack.
fn set_last_sync(app: &mut App, doc: crate::document::DocumentId, kind: SyncKind) {
    if let Some(d) = app.doc_mut(doc) {
        d.last_sync = Some(kind);
    }
}

/// Enqueues the store-side retraction of an abandoned merge's entry-time
/// resolve observation — the writer deletes the `origin='resolve'` row and
/// restores the baseline it superseded, so the next probe classifies the
/// still-diverged document truthfully. Mirrors the adopt enqueue's shape: a
/// store-less/degraded document simply skips it (there is no observation to
/// retract there either), and a failed enqueue degrades the whole store.
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
        }
        Err(e) => crate::materialize_ack::on_store_failure(app, e.to_string()),
    }
}

/// Cancels a `Pending` merge attempt with feedback (review fix F3) — the
/// disk-state round trip it was waiting on hasn't landed yet, so there is
/// no working form to restore and no `exit_in_place` contract to reuse; a
/// no-op outside `Pending`. Called from the same auto-exit transition sites
/// as `exit_in_place` (`workspace::switch_to`/`close_now`) when the
/// transitioning document has a merge attempt still `Pending` rather than
/// `Active`. The eventual `MergePrep` ack this cancels ahead of is not lost
/// track of — `handle_merge_prep_ack`'s own generation/doc ticket check
/// already treats an ack against a no-longer-`Pending` `app.merge` as stale
/// and drops it, exactly like a superseding `^M` would.
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

/// Dispatches `exit_in_place`/`cancel_pending` by `app.merge`'s current
/// state (review fix F3) — the one place the auto-exit transition sites
/// (`workspace::switch_to`/`close_now`) need to call, rather than each
/// re-deriving which of the two applies. A no-op on `Inactive`.
pub(crate) fn auto_exit(app: &mut App) {
    match &app.merge {
        MergeState::Active { .. } => exit_in_place(app),
        MergeState::Pending { .. } => cancel_pending(app),
        MergeState::Inactive => {}
    }
}

/// The save gate (plan WP4.S3, decision 6): while the resolver is active ON
/// the document being saved with unresolved blocks remaining, saving is
/// refused with the count — a reflexive save mid-merge must not publish a
/// half-resolved working form with zero friction. A rung in
/// `save::trigger_save`'s refusal ladder, so every save entry point (⌘S,
/// the guards' [S] answers, the quit fan-out) hits it structurally rather
/// than each call site remembering to ask. One rule, no content sniffing:
/// after exit or full resolution the document is ordinary dirty text and
/// saves normally, markers included.
pub(crate) fn refuses_save(app: &mut App, target: crate::document::DocumentId) -> bool {
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
        format!("{unresolved} conflict(s) to resolve — [O]urs [T]heirs [B]oth"),
    );
    true
}

/// `GlobalCommand::Merge`'s handler (plan WP3.S5): `^M` starts a merge
/// attempt when none is active, or exits an already-`Active` one in place —
/// a natural toggle, and the only reachable caller `exit_in_place` has
/// until later work packages (the resolver's own Esc, auto-exit) add
/// theirs.
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
            conflicts: blocks
                .iter()
                .map(|_| Conflict {
                    ours: "ours".to_string(),
                    theirs: "theirs".to_string(),
                })
                .collect(),
            blocks,
            cur: 0,
            saved_display_name: Some("saved-name".to_string()),
            theirs_obs: rune_db::ObsId::new(7).expect("nonzero"),
        }
    }

    /// A probe finding disk still diverged is not convergence — the
    /// `Active` merge, and any blocks resolved so far, are untouched.
    #[test]
    fn retract_active_on_convergence_is_a_noop_while_still_divergent() {
        let mut app = app_with("hello");
        let doc = app.active;
        app.merge = active_with_blocks(
            doc,
            vec![Block {
                start: 0,
                end: 5,
                resolved: false,
            }],
        );

        retract_active_on_convergence(&mut app, doc, SyncKind::Diverged);

        assert!(matches!(app.merge, MergeState::Active { .. }));
    }

    /// Any block already resolved means there is resolver progress to
    /// lose — convergence must not silently discard it, even once disk
    /// stops looking diverged.
    #[test]
    fn retract_active_on_convergence_is_a_noop_once_anything_is_resolved() {
        let mut app = app_with("hello");
        let doc = app.active;
        app.merge = active_with_blocks(
            doc,
            vec![
                Block {
                    start: 0,
                    end: 5,
                    resolved: true,
                },
                Block {
                    start: 5,
                    end: 9,
                    resolved: false,
                },
            ],
        );

        retract_active_on_convergence(&mut app, doc, SyncKind::Clean);

        assert!(matches!(app.merge, MergeState::Active { .. }));
    }

    /// Nothing resolved yet, and a later probe says disk no longer
    /// diverges: the merge exits cleanly with its own explanatory
    /// message, restoring the display name exactly like `exit_in_place`'s
    /// own unresolved-retirement path does.
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
                start: 0,
                end: 5,
                resolved: false,
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

    /// A convergence probe for a DIFFERENT document must never touch this
    /// one's active merge.
    #[test]
    fn retract_active_on_convergence_ignores_a_different_document() {
        let mut app = app_with("hello");
        let doc = app.active;
        app.merge = active_with_blocks(
            doc,
            vec![Block {
                start: 0,
                end: 5,
                resolved: false,
            }],
        );
        let other = app.open_document(Buffer::new("other"));

        retract_active_on_convergence(&mut app, other, SyncKind::Clean);

        assert!(matches!(app.merge, MergeState::Active { .. }));
    }
}
