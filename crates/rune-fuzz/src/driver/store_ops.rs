use std::sync::Arc;
use std::time::{Duration, SystemTime};

use rune_db::{ClockFn, Store};
use rune_tui::app::{self, App};
use rune_tui::db::{Db, DbBridge};
use rune_tui::runtime::{Effects, Msg};
use rune_tui::workspace;
use rune_vfs::{Vfs, VfsTestExt};

use crate::guard;
use crate::snapshot::Snapshot;

use super::discharge::drain_one_db_op;
use super::session::{Outcome, State};
use super::step_exec::step_and_check;

pub fn wait_for_db_op(bridge: &DbBridge, op_id: u64) -> rune_db::DbEvent {
    bridge.wait_for_bootstrap_event(|evt| match evt {
        rune_db::DbEvent::Ok { id, .. } | rune_db::DbEvent::Err { id, .. } => *id == op_id,
        rune_db::DbEvent::Fatal { .. } => true,
    })
}

pub(super) fn open_store(vfs: &Arc<dyn Vfs + Send + Sync>) -> (Arc<DbBridge>, Option<Db>) {
    let bridge = DbBridge::bootstrap();
    let clock: ClockFn = Arc::new(|| SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000));
    let db = Store::open_in_memory(clock, Arc::clone(vfs), bridge.on_event())
        .ok()
        .map(|store| Db::new(store, Arc::clone(&bridge), false));
    (bridge, db)
}

pub(super) fn drain_all_pending_setup(app: &mut App, bridge: &DbBridge) {
    while let Some(&op_id) = app.db_ops.keys().min() {
        let evt = wait_for_db_op(bridge, op_id);
        let mut effects = Effects::default();
        app::update(app, Msg::Db(evt), &mut effects);
    }
}

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

pub(super) fn diverge_disk(state: &mut State, prev: &mut Snapshot, outcome: &mut Outcome) -> bool {
    state.rediverge.note_external_write();
    state.disk_diverged_since_publish = true;
    state.diverge_step += 1;
    let bytes = format!("fuzz-external-write-{}\n", state.diverge_step).into_bytes();
    let path = state.path.clone();
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

    let violation = guard::catching_panic(|| {
        if switch_target != seed_doc {
            workspace::switch_to(&mut state.app, switch_target);
        }
        workspace::switch_to(&mut state.app, seed_doc);
    })
    .err();
    if let Some(v) = violation {
        outcome.violation = Some(v);
        outcome.final_snapshot = Some(prev.clone());
        outcome.final_ctx = None;
        return true;
    }
    *prev = Snapshot::capture(&mut state.app, false);
    false
}
