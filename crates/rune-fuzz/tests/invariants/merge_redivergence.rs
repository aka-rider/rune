use rune_fuzz::invariant::RedivergenceTracker;

use crate::support::{base_active_id, base_ctx, base_snapshot};

// ---------------------------------------------------------------------
// MERGE-NO-INSTANT-REDIVERGENCE (the stateful tracker)
// ---------------------------------------------------------------------

/// The `(prev, next)` pair of a merge completing on the active document:
/// `Active` retires to fully `Inactive` with a reconciled `BufferAhead`
/// classification — the transition that arms the tracker.
fn completion_pair() -> (rune_fuzz::snapshot::Snapshot, rune_fuzz::snapshot::Snapshot) {
    let mut prev = base_snapshot("merged");
    prev.merge_active = true;
    prev.merge_doc = Some(base_active_id());
    prev.merge_unresolved = 1;
    let mut next = base_snapshot("merged");
    next.active_last_sync = Some(rune_db::SyncKind::BufferAhead);
    (prev, next)
}

/// A later step whose snapshot re-classifies the same document `Diverged`.
fn rediverged_after(completed: &rune_fuzz::snapshot::Snapshot) -> rune_fuzz::snapshot::Snapshot {
    let mut next = completed.clone();
    next.active_last_sync = Some(rune_db::SyncKind::Diverged);
    next
}

#[test]
fn redivergence_tracker_detects_diverged_after_completion_with_no_external_write() {
    let mut tracker = RedivergenceTracker::default();
    let (prev, completed) = completion_pair();
    assert_eq!(tracker.observe(&prev, &completed, &base_ctx()), None);

    let rediverged = rediverged_after(&completed);
    let v = tracker
        .observe(&completed, &rediverged, &base_ctx())
        .expect("Diverged with no external write since completion must trip the tracker");
    assert_eq!(v.id, "MERGE-NO-INSTANT-REDIVERGENCE");
}

#[test]
fn redivergence_tracker_accepts_diverged_after_an_external_write() {
    let mut tracker = RedivergenceTracker::default();
    let (prev, completed) = completion_pair();
    assert_eq!(tracker.observe(&prev, &completed, &base_ctx()), None);

    tracker.note_external_write();
    let rediverged = rediverged_after(&completed);
    assert_eq!(tracker.observe(&completed, &rediverged, &base_ctx()), None);
}

#[test]
fn redivergence_tracker_accepts_diverged_after_an_undo_unwound_the_reconciliation() {
    let mut tracker = RedivergenceTracker::default();
    let (prev, completed) = completion_pair();
    assert_eq!(tracker.observe(&prev, &completed, &base_ctx()), None);

    let mut before_undo = completed.clone();
    before_undo.journal_pos = 5;
    let mut unwound = completed.clone();
    unwound.journal_pos = 4;
    assert_eq!(tracker.observe(&before_undo, &unwound, &base_ctx()), None);

    let rediverged = rediverged_after(&unwound);
    assert_eq!(tracker.observe(&unwound, &rediverged, &base_ctx()), None);
}

#[test]
fn redivergence_tracker_never_arms_on_an_escape_out_still_diverged() {
    let mut tracker = RedivergenceTracker::default();
    let (prev, mut escaped_out) = completion_pair();
    escaped_out.active_last_sync = Some(rune_db::SyncKind::Diverged);
    assert_eq!(tracker.observe(&prev, &escaped_out, &base_ctx()), None);

    let still_diverged = escaped_out.clone();
    assert_eq!(
        tracker.observe(&escaped_out, &still_diverged, &base_ctx()),
        None,
        "an Esc-out left truthfully Diverged must never arm the tracker"
    );
}

#[test]
fn redivergence_tracker_accepts_a_reconciled_classification_staying_put() {
    let mut tracker = RedivergenceTracker::default();
    let (prev, completed) = completion_pair();
    assert_eq!(tracker.observe(&prev, &completed, &base_ctx()), None);
    let still_reconciled = completed.clone();
    assert_eq!(
        tracker.observe(&completed, &still_reconciled, &base_ctx()),
        None
    );
}
