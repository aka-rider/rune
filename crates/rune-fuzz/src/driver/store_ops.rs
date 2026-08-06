//! Recovery-store setup/drain and the disk-divergence handler, split out of
//! `driver` (500-line budget): everything here either settles the store's
//! own async backlog or publishes bytes directly to the shared `Vfs` on
//! `State`'s behalf. `run` above reaches these through the unqualified
//! imports the parent module keeps.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use rune_db::{ClockFn, Store};
use rune_tui::app::{self, App};
use rune_tui::db::{Db, DbBridge};
use rune_tui::runtime::{Effects, Msg};
use rune_tui::workspace;
use rune_vfs::Vfs;

use crate::snapshot::Snapshot;

use super::step_exec::{drain_one_db_op, run_direct_catching_panic, step_and_check};
use super::{Outcome, State};

/// Blocks on `bridge` for the recovery-store reply completing `op_id` — the
/// one drain predicate every consumer of a buffered `DbEvent` shares,
/// whether it's this crate's own step execution/session setup or a test
/// that builds a session by hand and needs to feed the writer thread's
/// replies back in exactly as the real runtime does. A `DbEvent::Fatal`
/// always matches regardless of which id it's waiting on, since a
/// writer-thread fatal notice ends every outstanding op at once.
pub fn wait_for_db_op(bridge: &DbBridge, op_id: u64) -> rune_db::DbEvent {
    bridge.wait_for_bootstrap_event(|evt| match evt {
        rune_db::DbEvent::Ok { id, .. } | rune_db::DbEvent::Err { id, .. } => *id == op_id,
        rune_db::DbEvent::Fatal { .. } => true,
    })
}

/// Opens the in-memory recovery store this session's `App` is wired to, and
/// the `DbBridge` that routes its async replies back deterministically
/// (`State::bridge`'s own docs). Uses a fixed clock reading, never
/// `SystemTime::now` (WP3.S7 rule 7: zero wall-clock reads) — every session
/// shares the same instant, so the store's own session-establish/coalescing
/// logic stays exactly as reproducible as everything else this driver does.
/// `db` is `None` only if `Store::open_in_memory` itself failed; the
/// bridge is still returned so the caller's `State` always has one.
pub(super) fn open_store(vfs: &Arc<dyn Vfs + Send + Sync>) -> (Arc<DbBridge>, Option<Db>) {
    let bridge = DbBridge::bootstrap();
    let clock: ClockFn = Arc::new(|| SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000));
    let db = Store::open_in_memory(clock, Arc::clone(vfs), bridge.on_event())
        .ok()
        .map(|store| Db::new(store, Arc::clone(&bridge), false));
    (bridge, db)
}

/// Drains every recovery-store op enqueued by the session's own opening
/// `workspace::open_path` (the `Load` ack, and any `Probe`/scratch ops a
/// draft mint pulled in alongside it) before the driving loop starts —
/// unlike `drain_one_db_op`, this runs before `State` exists and isn't a
/// counted step: it settles the store's ONE synchronous-looking open, the
/// same way `App::relayout`/`sync_view` finish session setup without going
/// through `step_and_check` either.
pub(super) fn drain_all_pending_setup(app: &mut App, bridge: &DbBridge) {
    while let Some(&op_id) = app.db_ops.keys().min() {
        let evt = wait_for_db_op(bridge, op_id);
        let mut effects = Effects::default();
        app::update(app, Msg::Db(evt), &mut effects);
    }
}

/// `Action::DeliverDbAll`'s handler, also reused by `run`'s Rule 6d: drains
/// every recovery-store op pending right now, oldest first, one
/// `step_and_check` per op — `drain_one_db_op` repeated until nothing is
/// left or a violation stops the session. Returns `true` when the session
/// must stop. Bounded by however many ops are pending at the moment this
/// runs: draining never enqueues a fresh op of its own — a whole-backlog
/// flush where a `Db` ack lands through `db_dispatch`, which only ever
/// enqueues MORE ops from the merge-prep clean-fast-path's own
/// `resolve_adopt`, itself one-shot.
pub(super) fn drain_all_db_ops(
    state: &mut State,
    prev: &mut Snapshot,
    outcome: &mut Outcome,
) -> bool {
    let bridge = Arc::clone(&state.bridge);
    while outcome.violation.is_none() && !state.app.should_quit {
        let Some((msg, tag)) = drain_one_db_op(state, &bridge) else {
            return false;
        };
        if step_and_check(state, prev, msg, tag, None, outcome) {
            return true;
        }
    }
    false
}

/// `Action::DivergeDisk`'s handler: publishes fresh, deterministically-
/// varied bytes to the seeded document's own path directly on the shared
/// `Vfs` (an external editor's write, never routed through `update`), then
/// re-probes it through the same away-and-back reprobe the store-backed
/// merge tests use — a switch away from `seed_doc` (to the untitled draft,
/// if one is still open, else whichever other document is) followed by a
/// switch back, each funnelling through `workspace::switch_to`'s own probe
/// enqueue. Neither `switch_to` call goes through `update`, so both run
/// under `run_direct_catching_panic`'s own guard. Returns `true` when a
/// panic stopped the session.
pub(super) fn diverge_disk(state: &mut State, prev: &mut Snapshot, outcome: &mut Outcome) -> bool {
    state.diverge_step += 1;
    let bytes = format!("fuzz-external-write-{}\n", state.diverge_step).into_bytes();
    let path = state.path.clone();
    // `save_atomic` publishes through `write_durable` + `exchange`/
    // `rename_excl`, never removing `path` up front — an armed
    // `Action::FailNextSave` (`Mem::fail_next_save` targets `write_durable`)
    // fails before any mutation touches `path` at all, so a failed diverge
    // leaves the previously seeded bytes exactly as they were instead of
    // deleting them out from under the session.
    let _ = state.mem.save_atomic(&path, &bytes);

    let seed_doc = state.seed_doc;
    let switch_target = if state.draft_doc != seed_doc && state.app.doc(state.draft_doc).is_some() {
        state.draft_doc
    } else {
        state
            .app
            .documents
            .iter()
            .map(|(&id, _)| id)
            .find(|&id| id != seed_doc)
            .unwrap_or(seed_doc)
    };

    let violation = run_direct_catching_panic(&mut state.app, move |app| {
        if switch_target != seed_doc {
            workspace::switch_to(app, switch_target);
        }
        workspace::switch_to(app, seed_doc);
    });
    if let Some(v) = violation {
        outcome.violation = Some(v);
        outcome.final_snapshot = Some(prev.clone());
        outcome.final_ctx = None;
        return true;
    }
    *prev = Snapshot::capture(&mut state.app, false);
    false
}
