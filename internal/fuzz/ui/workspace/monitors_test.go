//go:build fuzzing

package workspace

import (
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/internal/fuzz/snapshot"
	"rune/pkg/ui/components/footer"
)

// unwindProbeMsg/FileSaveErrorMsg are local stand-ins whose %T
// ("workspace.unwindProbeMsg" / "workspace.FileSaveErrorMsg") is
// STRING-IDENTICAL to the real production types' — both this test's package
// and rune/pkg/ui/pages/workspace are named "workspace", and every %T-based
// check in this checker family (the established convention — see
// workspace.go's CheckTransition, footer.go's reflect usage) matches on
// exactly that string, not on Go type identity. This lets monitor tests
// exercise the real dispatch logic without an import cycle.
type unwindProbeMsg struct{}
type FileSaveErrorMsg struct{}

// ---- unwindRedetectMonitor ----

func TestUnwindRedetectMonitor_ArmsAndSettlesOnProbe(t *testing.T) {
	m := &unwindRedetectMonitor{}
	prev := snapshot.Snapshot{MergeActive: true}
	next := snapshot.Snapshot{MergeActive: false}
	undoMsg := tea.KeyPressMsg{Code: 'z', Mod: tea.ModSuper}

	if vs := m.Observe(prev, undoMsg, next); len(vs) != 0 {
		t.Fatalf("arming Observe() = %v, want no violations", vs)
	}
	if !m.pending {
		t.Fatal("expected monitor to arm on undo-triggered deactivation")
	}

	if vs := m.Observe(next, unwindProbeMsg{}, next); len(vs) != 0 {
		t.Fatalf("settle Observe() = %v, want no violations", vs)
	}
	if m.pending {
		t.Fatal("expected monitor to disarm once unwindProbeMsg lands, regardless of outcome")
	}
}

func TestUnwindRedetectMonitor_SettlesOnGuardOrMergeActive(t *testing.T) {
	prev := snapshot.Snapshot{MergeActive: true}
	deactivated := snapshot.Snapshot{MergeActive: false}
	undoMsg := tea.KeyPressMsg{Code: 'z', Mod: tea.ModCtrl}

	t.Run("guard reappears", func(t *testing.T) {
		m := &unwindRedetectMonitor{}
		m.Observe(prev, undoMsg, deactivated)
		guardVisible := snapshot.Snapshot{MergeActive: false, GuardVisible: true, GuardKind: footer.GuardMerge}
		if vs := m.Observe(deactivated, tea.WindowSizeMsg{}, guardVisible); len(vs) != 0 {
			t.Fatalf("Observe() = %v, want no violations", vs)
		}
		if m.pending {
			t.Fatal("expected monitor to disarm once the guard reappears")
		}
	})

	t.Run("redo re-enters resolver", func(t *testing.T) {
		m := &unwindRedetectMonitor{}
		m.Observe(prev, undoMsg, deactivated)
		reentered := snapshot.Snapshot{MergeActive: true}
		if vs := m.Observe(deactivated, tea.WindowSizeMsg{}, reentered); len(vs) != 0 {
			t.Fatalf("Observe() = %v, want no violations", vs)
		}
		if m.pending {
			t.Fatal("expected monitor to disarm once MergeActive is true again")
		}
	})
}

func TestUnwindRedetectMonitor_DisarmsOnEscape(t *testing.T) {
	m := &unwindRedetectMonitor{}
	prev := snapshot.Snapshot{MergeActive: true}
	next := snapshot.Snapshot{MergeActive: false}
	undoMsg := tea.KeyPressMsg{Code: 'z', Mod: tea.ModSuper}
	m.Observe(prev, undoMsg, next)
	if !m.pending {
		t.Fatal("setup: expected monitor armed before Escape")
	}

	esc := tea.KeyPressMsg{Code: tea.KeyEscape}
	if vs := m.Observe(next, esc, next); len(vs) != 0 {
		t.Fatalf("Observe(Escape) = %v, want no violations", vs)
	}
	if m.pending {
		t.Fatal("expected Escape to disarm the monitor (mergemode.Abort — no probe involved)")
	}
}

func TestUnwindRedetectMonitor_FiresAfterBudget(t *testing.T) {
	m := &unwindRedetectMonitor{}
	prev := snapshot.Snapshot{MergeActive: true}
	next := snapshot.Snapshot{MergeActive: false}
	undoMsg := tea.KeyPressMsg{Code: 'z', Mod: tea.ModSuper}
	m.Observe(prev, undoMsg, next)

	found := false
	for i := 0; i <= unwindRedetectMaxSteps+1; i++ {
		violations := m.Observe(next, tea.WindowSizeMsg{}, next)
		if len(violations) > 0 {
			if violations[0].InvariantID != "UNWIND-REDETECT" {
				t.Fatalf("violation = %+v, want InvariantID UNWIND-REDETECT", violations[0])
			}
			found = true
			break
		}
	}
	if !found {
		t.Fatalf("expected UNWIND-REDETECT to fire within %d steps of no settlement", unwindRedetectMaxSteps+1)
	}
}

// ---- quitSaveAllMonitor ----

func TestQuitSaveAllMonitor_ArmsAndSettlesOnAppQuitting(t *testing.T) {
	m := &quitSaveAllMonitor{}
	prev := snapshot.Snapshot{PendingDataLossKind: pendingKindQuit}
	saveMsg := footer.DataLossGuardResponseMsg{Response: footer.DataLossSave}
	next := snapshot.Snapshot{}

	if vs := m.Observe(prev, saveMsg, next); len(vs) != 0 {
		t.Fatalf("arming Observe() = %v, want no violations", vs)
	}
	if !m.pending {
		t.Fatal("expected monitor to arm on [S]ave response to the quit-batch guard")
	}

	quitting := snapshot.Snapshot{AppQuitting: true}
	if vs := m.Observe(next, tea.QuitMsg{}, quitting); len(vs) != 0 {
		t.Fatalf("settle Observe() = %v, want no violations", vs)
	}
	if m.pending {
		t.Fatal("expected monitor to disarm once AppQuitting is true")
	}
}

func TestQuitSaveAllMonitor_DoesNotArmOnDiscardOrNonQuit(t *testing.T) {
	m := &quitSaveAllMonitor{}
	next := snapshot.Snapshot{}

	// Save response, but NOT during a quit-batch guard.
	prevNotQuit := snapshot.Snapshot{PendingDataLossKind: pendingKindClose}
	saveMsg := footer.DataLossGuardResponseMsg{Response: footer.DataLossSave}
	m.Observe(prevNotQuit, saveMsg, next)
	if m.pending {
		t.Fatal("must not arm: PendingDataLossKind was Close, not Quit")
	}

	// Discard response during a quit-batch guard — not the Save trigger.
	prevQuit := snapshot.Snapshot{PendingDataLossKind: pendingKindQuit}
	discardMsg := footer.DataLossGuardResponseMsg{Response: footer.DataLossDiscard}
	m.Observe(prevQuit, discardMsg, next)
	if m.pending {
		t.Fatal("must not arm: response was Discard, not Save")
	}
}

func TestQuitSaveAllMonitor_DisarmsOnSaveError(t *testing.T) {
	m := &quitSaveAllMonitor{}
	prev := snapshot.Snapshot{PendingDataLossKind: pendingKindQuit}
	saveMsg := footer.DataLossGuardResponseMsg{Response: footer.DataLossSave}
	next := snapshot.Snapshot{}
	m.Observe(prev, saveMsg, next)
	if !m.pending {
		t.Fatal("setup: expected monitor armed before the save error")
	}

	if vs := m.Observe(next, FileSaveErrorMsg{}, next); len(vs) != 0 {
		t.Fatalf("Observe(FileSaveErrorMsg) = %v, want no violations (abort is correct — QUIT-ABORT owns it)", vs)
	}
	if m.pending {
		t.Fatal("expected FileSaveErrorMsg to disarm the monitor (a correct quit abort, not a liveness failure)")
	}
}

// TestQuitSaveAllMonitor_DisarmsOnSynchronousAbort covers the false positive
// found via FuzzHumanSession (WP7 follow-up): saveAllDirtyForQuit
// (workspace_quit.go) has FOUR early-return abort paths — a §1.4.8
// re-derived SyncDiverged doc, a failed divergence re-check, a missing CAS
// baseline, and a VFS-reconstruction failure — that clear pendingDataLoss
// SYNCHRONOUSLY, in the very same Update call that processes the [S]ave
// response, WITHOUT ever issuing an async save or emitting
// FileSaveErrorMsg. The monitor must disarm on that transition too, not
// just on FileSaveErrorMsg.
func TestQuitSaveAllMonitor_DisarmsOnSynchronousAbort(t *testing.T) {
	m := &quitSaveAllMonitor{}
	prev := snapshot.Snapshot{PendingDataLossKind: pendingKindQuit}
	saveMsg := footer.DataLossGuardResponseMsg{Response: footer.DataLossSave}
	m.Observe(prev, saveMsg, snapshot.Snapshot{PendingDataLossKind: pendingKindQuit})
	if !m.pending {
		t.Fatal("setup: expected monitor armed before the synchronous abort")
	}

	// The SAME Update call that processed the [S]ave response also detected
	// SyncDiverged and aborted the quit — pendingDataLoss cleared to kind
	// None, with no FileSaveErrorMsg anywhere in this transition.
	aborted := snapshot.Snapshot{PendingDataLossKind: pendingKindNone}
	if vs := m.Observe(snapshot.Snapshot{PendingDataLossKind: pendingKindQuit}, footer.ShowStatusMsg{}, aborted); len(vs) != 0 {
		t.Fatalf("Observe(synchronous abort) = %v, want no violations", vs)
	}
	if m.pending {
		t.Fatal("expected a synchronous kind-cleared abort to disarm the monitor (correct refusal, not a liveness failure)")
	}
}

func TestQuitSaveAllMonitor_FiresAfterBudget(t *testing.T) {
	m := &quitSaveAllMonitor{}
	prev := snapshot.Snapshot{PendingDataLossKind: pendingKindQuit}
	saveMsg := footer.DataLossGuardResponseMsg{Response: footer.DataLossSave}
	// PendingDataLossKind stays Quit throughout — matches production, where
	// a still-in-flight multi-doc batch decrements pendingDataLoss.saveLeft
	// without ever reassigning .kind away from actionQuit (workspace_io_
	// handlers.go). A zero-value Snapshot here (kind==pendingKindNone) would
	// make the monitor's own abandoned-quit disarm fire on step 1.
	next := snapshot.Snapshot{PendingDataLossKind: pendingKindQuit}
	m.Observe(prev, saveMsg, next)

	found := false
	for i := 0; i <= quitSaveAllMaxSteps+1; i++ {
		violations := m.Observe(next, tea.WindowSizeMsg{}, next)
		if len(violations) > 0 {
			if violations[0].InvariantID != "QUIT-SAVEALL" {
				t.Fatalf("violation = %+v, want InvariantID QUIT-SAVEALL", violations[0])
			}
			found = true
			break
		}
	}
	if !found {
		t.Fatalf("expected QUIT-SAVEALL to fire within %d steps of no AppQuitting", quitSaveAllMaxSteps+1)
	}
}
