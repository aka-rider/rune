//! Unit tests for `SAVE-AGREES-WITH-DIVERGENCE` (issue #65). Same
//! controlled-experiment pattern as every other file here: one hand-built
//! BAD step sequence asserting the tracker fires with the right id, and
//! well-formed companions of the same shape — an authorized force, a
//! reconciled verdict, a refused attempt, a stale chrome classification the
//! store itself disagrees with — asserting `None`.

use rune_db::{DbEvent, MaterializePrep, OpOutcome, SyncKind};
use rune_fuzz::invariant::DivergentSaveTracker;
use rune_fuzz::snapshot::Snapshot;
use rune_tui::guard::GuardKind;
use rune_tui::runtime::Msg;

use crate::support::{base_active_id, base_ctx, base_snapshot};

fn idle() -> Snapshot {
    let mut snap = base_snapshot("buffer");
    snap.save_in_flight_by_doc.insert(base_active_id(), false);
    snap.saved_version_by_doc.insert(base_active_id(), 1);
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
    snap.saved_version_by_doc.insert(base_active_id(), 2);
    snap
}

fn prepare_ack(sync: SyncKind) -> Msg {
    Msg::Db(DbEvent::Ok {
        id: 1,
        result: OpOutcome::MaterializePrep(Box::new(MaterializePrep::Overwrite {
            bound_path: String::new(),
            expect_hash: String::new(),
            sync,
        })),
    })
}

#[test]
fn divergent_save_tracker_detects_a_commit_the_prepare_verdict_forbade() {
    let mut tracker = DivergentSaveTracker::default();
    let idle = idle();
    let armed = armed_from(&idle);
    assert_eq!(tracker.observe(&idle, &armed, &base_ctx()), None);

    tracker.note_prepare_ack(&prepare_ack(SyncKind::Diverged), Some(base_active_id()), 2);
    let committed = committed_from(&armed);
    let v = tracker
        .observe(&armed, &committed, &base_ctx())
        .expect("an unforced commit past a disk-divergent verdict must trip the tracker");
    assert_eq!(v.id, "SAVE-AGREES-WITH-DIVERGENCE");
}

#[test]
fn divergent_save_tracker_accepts_a_commit_the_disk_conflict_guard_authorized() {
    let mut tracker = DivergentSaveTracker::default();
    let mut idle = idle();
    idle.guard = Some((base_active_id(), GuardKind::DiskConflict));
    let armed = armed_from(&idle);
    assert_eq!(tracker.observe(&idle, &armed, &base_ctx()), None);

    tracker.note_prepare_ack(&prepare_ack(SyncKind::Diverged), Some(base_active_id()), 2);
    let committed = committed_from(&armed);
    assert_eq!(
        tracker.observe(&armed, &committed, &base_ctx()),
        None,
        "[S]ave anyway is exactly the authorization this invariant asks for"
    );
}

#[test]
fn divergent_save_tracker_accepts_a_commit_from_a_reconciled_verdict() {
    let mut tracker = DivergentSaveTracker::default();
    let idle = idle();
    let armed = armed_from(&idle);
    assert_eq!(tracker.observe(&idle, &armed, &base_ctx()), None);

    tracker.note_prepare_ack(
        &prepare_ack(SyncKind::BufferAhead),
        Some(base_active_id()),
        2,
    );
    let committed = committed_from(&armed);
    assert_eq!(tracker.observe(&armed, &committed, &base_ctx()), None);
}

/// The chrome's own `last_sync` is seeded conservatively by any save-time
/// refusal and stays `Diverged` until something re-classifies the document,
/// so a later save the store itself green-lights would fire a tracker that
/// read that cached field instead of the verdict the gate decided on.
#[test]
fn divergent_save_tracker_ignores_a_stale_diverged_classification() {
    let mut tracker = DivergentSaveTracker::default();
    let mut idle = idle();
    idle.active_last_sync = Some(SyncKind::Diverged);
    let mut armed = armed_from(&idle);
    armed.active_last_sync = Some(SyncKind::Diverged);
    assert_eq!(tracker.observe(&idle, &armed, &base_ctx()), None);

    tracker.note_prepare_ack(
        &prepare_ack(SyncKind::BufferAhead),
        Some(base_active_id()),
        2,
    );
    let mut committed = committed_from(&armed);
    committed.active_last_sync = Some(SyncKind::Diverged);
    assert_eq!(
        tracker.observe(&armed, &committed, &base_ctx()),
        None,
        "the gate decides on the store's fresh verdict, never on the chrome's cached one"
    );
}

#[test]
fn divergent_save_tracker_accepts_an_attempt_that_never_committed() {
    let mut tracker = DivergentSaveTracker::default();
    let idle = idle();
    let armed = armed_from(&idle);
    assert_eq!(tracker.observe(&idle, &armed, &base_ctx()), None);

    tracker.note_prepare_ack(&prepare_ack(SyncKind::Diverged), Some(base_active_id()), 2);
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

/// One tracker watches every document at once: a step that arms a second
/// document's save must not drop the violation the first one's resolution
/// carries.
#[test]
fn divergent_save_tracker_reports_a_violation_while_another_save_arms() {
    let other = crate::support::other_doc_id();
    let mut tracker = DivergentSaveTracker::default();
    let mut idle = idle();
    idle.save_in_flight_by_doc.insert(other, false);
    idle.saved_version_by_doc.insert(other, 1);
    let armed = armed_from(&idle);
    assert_eq!(tracker.observe(&idle, &armed, &base_ctx()), None);

    tracker.note_prepare_ack(&prepare_ack(SyncKind::Diverged), Some(base_active_id()), 2);
    let mut committed = committed_from(&armed);
    committed.save_in_flight_by_doc.insert(other, true);
    let v = tracker
        .observe(&armed, &committed, &base_ctx())
        .expect("the resolution must be checked even on a step that arms another document");
    assert_eq!(v.id, "SAVE-AGREES-WITH-DIVERGENCE");
}
