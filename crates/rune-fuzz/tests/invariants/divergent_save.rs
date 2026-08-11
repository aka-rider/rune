//! Unit tests for `SAVE-AGREES-WITH-DIVERGENCE` (issue #65). Same
//! controlled-experiment pattern as every other file here: one hand-built
//! BAD step sequence asserting the tracker fires with the right id, and
//! well-formed companions of the same shape — an authorized force, a
//! reconciled classification, a refused attempt — asserting `None`.

use rune_fuzz::invariant::DivergentSaveTracker;
use rune_fuzz::snapshot::Snapshot;
use rune_tui::guard::GuardKind;

use crate::support::{base_active_id, base_ctx, base_snapshot};

fn idle(sync: Option<rune_db::SyncKind>) -> Snapshot {
    let mut snap = base_snapshot("buffer");
    snap.active_last_sync = sync;
    snap.save_in_flight_by_doc.insert(base_active_id(), false);
    snap
}

fn armed_from(idle: &Snapshot) -> Snapshot {
    let mut snap = idle.clone();
    snap.guard = None;
    snap.save_in_flight_by_doc.insert(base_active_id(), true);
    snap
}

fn committed_from(armed: &Snapshot) -> Snapshot {
    let mut snap = armed.clone();
    snap.save_in_flight_by_doc.insert(base_active_id(), false);
    snap.saved_version = armed.saved_version + 1;
    snap
}

#[test]
fn divergent_save_tracker_detects_a_commit_the_classification_forbade() {
    let mut tracker = DivergentSaveTracker::default();
    let idle = idle(Some(rune_db::SyncKind::Diverged));
    let armed = armed_from(&idle);
    assert_eq!(tracker.observe(&idle, &armed, &base_ctx()), None);

    let committed = committed_from(&armed);
    let v = tracker
        .observe(&armed, &committed, &base_ctx())
        .expect("an unforced commit against a diverged classification must trip the tracker");
    assert_eq!(v.id, "SAVE-AGREES-WITH-DIVERGENCE");
}

#[test]
fn divergent_save_tracker_accepts_a_commit_the_disk_conflict_guard_authorized() {
    let mut tracker = DivergentSaveTracker::default();
    let mut idle = idle(Some(rune_db::SyncKind::Diverged));
    idle.guard = Some((base_active_id(), GuardKind::DiskConflict));
    let armed = armed_from(&idle);
    assert_eq!(tracker.observe(&idle, &armed, &base_ctx()), None);

    let committed = committed_from(&armed);
    assert_eq!(
        tracker.observe(&armed, &committed, &base_ctx()),
        None,
        "[S]ave anyway is exactly the authorization this invariant asks for"
    );
}

#[test]
fn divergent_save_tracker_accepts_a_commit_from_a_reconciled_classification() {
    let mut tracker = DivergentSaveTracker::default();
    let idle = idle(Some(rune_db::SyncKind::BufferAhead));
    let armed = armed_from(&idle);
    assert_eq!(tracker.observe(&idle, &armed, &base_ctx()), None);

    let committed = committed_from(&armed);
    assert_eq!(tracker.observe(&armed, &committed, &base_ctx()), None);
}

#[test]
fn divergent_save_tracker_accepts_an_attempt_that_never_committed() {
    let mut tracker = DivergentSaveTracker::default();
    let idle = idle(Some(rune_db::SyncKind::DiskAhead));
    let armed = armed_from(&idle);
    assert_eq!(tracker.observe(&idle, &armed, &base_ctx()), None);

    let mut refused = armed.clone();
    refused
        .save_in_flight_by_doc
        .insert(base_active_id(), false);
    assert_eq!(
        tracker.observe(&armed, &refused, &base_ctx()),
        None,
        "a refused attempt resolves without advancing saved_version"
    );
}
