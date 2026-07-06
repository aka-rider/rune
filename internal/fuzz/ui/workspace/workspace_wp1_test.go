//go:build fuzzing

package workspace

import (
	"testing"

	"rune/internal/fuzz/snapshot"
	"rune/pkg/ui/components/footer"
	pgworkspace "rune/pkg/ui/pages/workspace"
)

// ---- GUARD-STATE-COH (L0) ----

func TestGuardStateCoh_NoFalsePositive_Hidden(t *testing.T) {
	// No guard visible at all — GUARD-STATE-COH never applies.
	s := snapshot.Snapshot{GuardVisible: false, GuardKind: footer.GuardMerge}
	if v := Check(s); v != nil && v.InvariantID == "GUARD-STATE-COH" {
		t.Errorf("Check() = %v, want nil (guard not visible)", v)
	}
}

func TestGuardStateCoh_NoFalsePositive_EachKindCorrelated(t *testing.T) {
	cases := []snapshot.Snapshot{
		{GuardVisible: true, GuardKind: footer.GuardMerge, PendingConflictActive: true},
		{GuardVisible: true, GuardKind: footer.GuardDeleted, PendingDeletedActive: true},
		{GuardVisible: true, GuardKind: footer.GuardRaced, PendingRacedActive: true},
		{GuardVisible: true, GuardKind: footer.GuardTrash, PendingDataLossKind: pendingKindTrash},
		{GuardVisible: true, GuardKind: footer.GuardDirty, PendingDataLossKind: pendingKindClose},
		{GuardVisible: true, GuardKind: footer.GuardDirty, PendingDataLossKind: pendingKindQuit},
		{GuardVisible: true, GuardKind: footer.GuardDirty, PendingDataLossKind: pendingKindEvict},
		{GuardVisible: true, GuardKind: footer.GuardDegraded, StoreDegraded: true},
	}
	for i, s := range cases {
		if v := Check(s); v != nil {
			t.Errorf("case %d: Check() = %v, want nil", i, v)
		}
	}
}

func TestGuardStateCoh_DetectsCorruption(t *testing.T) {
	cases := []snapshot.Snapshot{
		{GuardVisible: true, GuardKind: footer.GuardMerge, PendingConflictActive: false},
		{GuardVisible: true, GuardKind: footer.GuardDeleted, PendingDeletedActive: false},
		{GuardVisible: true, GuardKind: footer.GuardRaced, PendingRacedActive: false},
		{GuardVisible: true, GuardKind: footer.GuardTrash, PendingDataLossKind: pendingKindNone},
		{GuardVisible: true, GuardKind: footer.GuardDirty, PendingDataLossKind: pendingKindNone},
		{GuardVisible: true, GuardKind: footer.GuardDirty, PendingDataLossKind: pendingKindTrash},
		{GuardVisible: true, GuardKind: footer.GuardDegraded, StoreDegraded: false},
	}
	for i, s := range cases {
		v := Check(s)
		if v == nil || v.InvariantID != "GUARD-STATE-COH" {
			t.Errorf("case %d: Check() = %v, want GUARD-STATE-COH", i, v)
		}
	}
}

// ---- GUARD-RESPONSE-CLEARS (L1) ----

func TestGuardResponseClears_NoFalsePositive_Cleared(t *testing.T) {
	prev := snapshot.Snapshot{GuardVisible: true, GuardKind: footer.GuardMerge, PendingConflictActive: true}
	next := snapshot.Snapshot{GuardVisible: false, PendingConflictActive: false}
	msg := footer.DataLossGuardResponseMsg{Response: footer.DataLossDiscard}
	vs := CheckTransition(prev, msg, next)
	for _, v := range vs {
		if v.InvariantID == "GUARD-RESPONSE-CLEARS" {
			t.Errorf("CheckTransition() = %v, want no GUARD-RESPONSE-CLEARS", v)
		}
	}
}

// TestGuardResponseClears_NoFalsePositive_FreshReraise pins the real WP0 soak
// finding (corpus seed 051d236c9d43269c): [S]ave on a dirty-close guard
// re-enters startSave, whose own §1.4.8 pre-write divergence re-check can
// legitimately re-raise GuardMerge SYNCHRONOUSLY, within the same Update call
// that processes this very DataLossGuardResponseMsg. That is a visibly
// correlated FRESH conflict, not a lingering stale flag.
func TestGuardResponseClears_NoFalsePositive_FreshReraise(t *testing.T) {
	prev := snapshot.Snapshot{GuardVisible: true, GuardKind: footer.GuardDirty, PendingDataLossKind: pendingKindClose}
	next := snapshot.Snapshot{GuardVisible: true, GuardKind: footer.GuardMerge, PendingConflictActive: true}
	msg := footer.DataLossGuardResponseMsg{Response: footer.DataLossSave}
	vs := CheckTransition(prev, msg, next)
	for _, v := range vs {
		if v.InvariantID == "GUARD-RESPONSE-CLEARS" {
			t.Errorf("CheckTransition() = %v, want no GUARD-RESPONSE-CLEARS (fresh correlated re-raise)", v)
		}
	}
}

func TestGuardResponseClears_DetectsLingeringFlag(t *testing.T) {
	// PendingConflictActive stuck true with NO correlated GuardMerge visible
	// (guard cleared or showing something else) — the dangerous "misroutes a
	// later dirty-guard Discard into load theirs" case.
	prev := snapshot.Snapshot{GuardVisible: true, GuardKind: footer.GuardMerge, PendingConflictActive: true}
	next := snapshot.Snapshot{GuardVisible: false, PendingConflictActive: true}
	msg := footer.DataLossGuardResponseMsg{Response: footer.DataLossDiscard}
	vs := CheckTransition(prev, msg, next)
	found := false
	for _, v := range vs {
		if v.InvariantID == "GUARD-RESPONSE-CLEARS" {
			found = true
		}
	}
	if !found {
		t.Errorf("CheckTransition() = %v, want GUARD-RESPONSE-CLEARS", vs)
	}
}

// ---- MERGE-GUARD-RAISE / DELETED-GUARD-RAISE / QUIT-ABORT (L1) ----

func correlatedPrev() snapshot.Snapshot {
	return snapshot.Snapshot{
		SaveInFlight:   true,
		SaveRequestID:  "req-1",
		DocID:          2,
		ActiveFilePath: "/fuzz/a.md",
	}
}

func TestMergeGuardRaise_NoFalsePositive(t *testing.T) {
	prev := correlatedPrev()
	next := snapshot.Snapshot{GuardVisible: true, GuardKind: footer.GuardMerge, PendingConflictActive: true}
	msg := pgworkspace.FileSaveErrorMsg{DocID: 2, RequestID: "req-1", Conflict: true}
	vs := CheckTransition(prev, msg, next)
	for _, v := range vs {
		if v.InvariantID == "MERGE-GUARD-RAISE" {
			t.Errorf("CheckTransition() = %v, want no MERGE-GUARD-RAISE", v)
		}
	}
}

func TestMergeGuardRaise_DetectsMissingGuard(t *testing.T) {
	prev := correlatedPrev()
	next := snapshot.Snapshot{GuardVisible: false} // guard never raised — bug
	msg := pgworkspace.FileSaveErrorMsg{DocID: 2, RequestID: "req-1", Conflict: true}
	vs := CheckTransition(prev, msg, next)
	found := false
	for _, v := range vs {
		if v.InvariantID == "MERGE-GUARD-RAISE" {
			found = true
		}
	}
	if !found {
		t.Errorf("CheckTransition() = %v, want MERGE-GUARD-RAISE", vs)
	}
}

// TestMergeGuardRaise_ExcludesUncorrelatedSave pins critic R1: a quit-batch/
// evict background save's ack for the SAME DocID must NOT be misread as
// THIS interactive save's conflict merely because an interactive save is
// concurrently in flight — the RequestID must also match.
func TestMergeGuardRaise_ExcludesUncorrelatedSave(t *testing.T) {
	prev := correlatedPrev() // SaveRequestID == "req-1"
	next := snapshot.Snapshot{GuardVisible: false}
	msg := pgworkspace.FileSaveErrorMsg{DocID: 2, RequestID: "quit-batch-req", Conflict: true}
	vs := CheckTransition(prev, msg, next)
	for _, v := range vs {
		if v.InvariantID == "MERGE-GUARD-RAISE" {
			t.Errorf("CheckTransition() = %v, want no MERGE-GUARD-RAISE (uncorrelated RequestID)", v)
		}
	}
}

func TestDeletedGuardRaise_NoFalsePositive(t *testing.T) {
	prev := correlatedPrev()
	next := snapshot.Snapshot{GuardVisible: true, GuardKind: footer.GuardDeleted, PendingDeletedActive: true}
	msg := pgworkspace.FileSaveErrorMsg{DocID: 2, RequestID: "req-1", Missing: true}
	vs := CheckTransition(prev, msg, next)
	for _, v := range vs {
		if v.InvariantID == "DELETED-GUARD-RAISE" {
			t.Errorf("CheckTransition() = %v, want no DELETED-GUARD-RAISE", v)
		}
	}
}

func TestDeletedGuardRaise_DetectsMissingGuard(t *testing.T) {
	prev := correlatedPrev()
	next := snapshot.Snapshot{GuardVisible: false}
	msg := pgworkspace.FileSaveErrorMsg{DocID: 2, RequestID: "req-1", Missing: true}
	vs := CheckTransition(prev, msg, next)
	found := false
	for _, v := range vs {
		if v.InvariantID == "DELETED-GUARD-RAISE" {
			found = true
		}
	}
	if !found {
		t.Errorf("CheckTransition() = %v, want DELETED-GUARD-RAISE", vs)
	}
}

func TestQuitAbort_NoFalsePositive(t *testing.T) {
	prev := snapshot.Snapshot{PendingDataLossKind: pendingKindQuit}
	next := snapshot.Snapshot{PendingDataLossKind: pendingKindNone, AppQuitting: false}
	msg := pgworkspace.FileSaveErrorMsg{DocID: 3, RequestID: "quit-1"}
	vs := CheckTransition(prev, msg, next)
	for _, v := range vs {
		if v.InvariantID == "QUIT-ABORT" {
			t.Errorf("CheckTransition() = %v, want no QUIT-ABORT", v)
		}
	}
}

func TestQuitAbort_DetectsStuckPending(t *testing.T) {
	prev := snapshot.Snapshot{PendingDataLossKind: pendingKindQuit}
	next := snapshot.Snapshot{PendingDataLossKind: pendingKindQuit}
	msg := pgworkspace.FileSaveErrorMsg{DocID: 3, RequestID: "quit-1"}
	vs := CheckTransition(prev, msg, next)
	found := false
	for _, v := range vs {
		if v.InvariantID == "QUIT-ABORT" {
			found = true
		}
	}
	if !found {
		t.Errorf("CheckTransition() = %v, want QUIT-ABORT", vs)
	}
}

func TestQuitAbort_DetectsQuitProceeding(t *testing.T) {
	prev := snapshot.Snapshot{PendingDataLossKind: pendingKindQuit}
	next := snapshot.Snapshot{PendingDataLossKind: pendingKindNone, AppQuitting: true}
	msg := pgworkspace.FileSaveErrorMsg{DocID: 3, RequestID: "quit-1"}
	vs := CheckTransition(prev, msg, next)
	found := false
	for _, v := range vs {
		if v.InvariantID == "QUIT-ABORT" {
			found = true
		}
	}
	if !found {
		t.Errorf("CheckTransition() = %v, want QUIT-ABORT", vs)
	}
}
