//! Merge mode (plan "merge-user-s-changes-with-idempotent-octopus", WP3):
//! entry (`begin`), the `MergePrep` ack landing (`landing`), and the
//! Esc-exit-in-place chokepoint (`exit_in_place`). The resolver's own keys/
//! navigation/accept (WP4), painting (WP5), and resync/auto-exit/guard
//! (WP6) are later work packages layered on the state this module owns.

pub mod frame;
pub mod keys;
mod landing;
pub(crate) mod paint;
mod resolve;
pub mod state;

pub use keys::{MERGE_BINDINGS, MergeCommand};
pub(crate) use landing::handle_merge_prep_ack;
pub use state::{Block, Conflict, MergeIntent, MergeState};

use rune_db::SyncKind;

use crate::app::{App, StatusSource};
use crate::db::PendingOp;
use crate::runtime::Effects;

/// `⌘M` (plan WP3.S5): a fast pre-check against `Document.last_sync`
/// (render/hint state only, plan Gotchas `[R3]`) before ever enqueueing a
/// `MergePrep` — refuses immediately, with feedback, when there is
/// obviously nothing to merge, without waiting on a round trip to confirm
/// what the hint already rules out. The fresh `MergePrep` landing
/// (`landing::handle_merge_prep_ack`) re-checks authoritatively before
/// actually installing anything.
pub(crate) fn begin(app: &mut App, intent: MergeIntent, _effects: &mut Effects) {
    let id = app.active;
    let Some(doc) = app.doc(id) else { return };
    if !matches!(
        doc.last_sync,
        Some(SyncKind::DiskAhead) | Some(SyncKind::Diverged)
    ) {
        app.set_status("no divergence to merge", StatusSource::Other);
        return;
    }
    let Some(db_id) = doc.db.as_ref().map(|d| d.db_id) else {
        app.set_status("no divergence to merge", StatusSource::Other);
        return;
    };
    let Some(db) = app.db.as_ref() else {
        app.set_status("no divergence to merge", StatusSource::Other);
        return;
    };
    if db.degraded {
        app.set_status("no divergence to merge", StatusSource::Other);
        return;
    }

    match db.store.merge_prep(db_id) {
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
/// place — no abort-restore"). A no-op outside `Active` (`Inactive`/
/// `Pending` have no `saved_display_name` to restore and no blocks to
/// count). `pub(crate)`'s only caller in THIS work package is `⌘M` toggling
/// off an already-`Active` merge by hand; later work packages (auto-exit on
/// tab switch/close/quit, the resolver's own Esc, fully-resolved auto-exit)
/// add more callers without changing this function's contract.
pub(crate) fn exit_in_place(app: &mut App) {
    let MergeState::Active {
        doc,
        blocks,
        saved_display_name,
        ..
    } = std::mem::take(&mut app.merge)
    else {
        return;
    };
    if let Some(d) = app.doc_mut(doc) {
        d.display_name = saved_display_name;
    }
    let unresolved = blocks.iter().filter(|b| !b.resolved).count();
    let message = if unresolved == 0 {
        "merge complete — \u{2318}S to save".to_string()
    } else {
        format!("merge closed — {unresolved} unresolved marker block(s) remain")
    };
    app.set_status(message, StatusSource::Other);
}

/// The `⌘S` gate (plan WP4.S3, decision 6): while the resolver is active ON
/// the document being saved with unresolved blocks remaining, saving is
/// refused with the count — a reflexive save mid-merge must not publish a
/// half-resolved working form with zero friction. One rule, no content
/// sniffing: after exit or full resolution the document is ordinary dirty
/// text and saves normally, markers included.
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
    app.set_status(
        format!("{unresolved} conflict(s) to resolve — [O]urs [T]heirs [B]oth"),
        StatusSource::Other,
    );
    true
}

/// `GlobalCommand::Merge`'s handler (plan WP3.S5): `⌘M` starts a merge
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
