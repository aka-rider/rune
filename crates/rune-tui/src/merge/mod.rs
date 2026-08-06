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
pub(crate) mod resync;
pub mod state;

pub use keys::{MERGE_BINDINGS, MergeCommand};
pub(crate) use landing::handle_merge_prep_ack;
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
    if doc.save_in_flight {
        messages::warn(app, "save in progress — merge after it completes");
        return;
    }
    if !matches!(
        doc.last_sync,
        Some(SyncKind::DiskAhead) | Some(SyncKind::Diverged)
    ) {
        messages::warn(app, "no divergence to merge");
        return;
    }
    let Some(db_id) = doc.db.as_ref().map(|d| d.db_id) else {
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
        ..
    } = std::mem::take(&mut app.merge)
    else {
        return;
    };
    let unresolved = blocks.iter().filter(|b| !b.resolved).count();
    if let Some(d) = app.doc_mut(doc) {
        d.display_name = saved_display_name;
        if unresolved == 0 {
            // A completed merge (including all-[B]oth, whose kept markers
            // the user explicitly chose) leaves the buffer strictly ahead
            // of the disk bytes it just reconciled with — recording that
            // here is what retires the disk-changed banner/hint instead of
            // re-inviting a merge forever. Esc-out with unresolved blocks
            // leaves `last_sync` untouched: the document is still
            // truthfully diverged and the affordances must return.
            d.last_sync = Some(SyncKind::BufferAhead);
        }
    }
    let message = if unresolved == 0 {
        "merge complete — \u{2318}S to save".to_string()
    } else {
        format!("merge closed — {unresolved} unresolved marker block(s) remain")
    };
    messages::info(app, message);
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
