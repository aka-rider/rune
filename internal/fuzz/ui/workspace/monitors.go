//go:build fuzzing

package workspace

import (
	"fmt"

	tea "charm.land/bubbletea/v2"

	"rune/internal/fuzz/invariant"
	"rune/internal/fuzz/snapshot"
	pgworkspace "rune/pkg/ui/pages/workspace"
)

// NewMonitors returns a fresh set of all workspace L2 monitors for one Run call.
func NewMonitors() []invariant.Monitor {
	return []invariant.Monitor{
		&quitLivenessMonitor{},
		&undoRedoMonitor{},
		&extNoClobberMonitor{},
	}
}

// ---- F1: quit liveness monitor ----
// After footer.ConfirmQuitMsg fires without a guard blocking it, AppQuitting
// must become true within N settled steps.

const quitMaxSteps = 20

type quitLivenessMonitor struct {
	pending bool
	steps   int
}

func (m *quitLivenessMonitor) Reset() { m.pending = false; m.steps = 0 }

func (m *quitLivenessMonitor) Observe(_ snapshot.Snapshot, msg tea.Msg, next snapshot.Snapshot) []invariant.Violation {
	typeName := fmt.Sprintf("%T", msg)

	// Satisfied: quit was processed.
	if next.AppQuitting {
		m.pending = false
		m.steps = 0
		return nil
	}

	// Trigger: ConfirmQuitMsg while not guard-blocked.
	if typeName == "footer.ConfirmQuitMsg" && !next.GuardVisible {
		m.pending = true
		m.steps = 0
		return nil
	}

	if m.pending {
		m.steps++
		if m.steps > quitMaxSteps {
			m.pending = false
			return []invariant.Violation{{
				InvariantID: "F1",
				Message: fmt.Sprintf(
					"quit not processed within %d steps after ConfirmQuitMsg", quitMaxSteps,
				),
			}}
		}
	}
	return nil
}

// ---- DL3: undo→redo round-trip monitor ----
// When undo changes Content, record the pre-undo content.
// When the immediately following redo fires, assert the content is restored.

type undoRedoMonitor struct {
	postUndo          bool
	contentBeforeUndo string
}

func (m *undoRedoMonitor) Reset() { m.postUndo = false; m.contentBeforeUndo = "" }

func (m *undoRedoMonitor) Observe(prev snapshot.Snapshot, msg tea.Msg, next snapshot.Snapshot) []invariant.Violation {
	km, ok := msg.(tea.KeyPressMsg)
	if !ok {
		m.postUndo = false // non-key event interrupts the round-trip window
		return nil
	}

	isUndo := km.Code == 'z' && km.Mod == tea.ModSuper
	isRedo := km.Code == 'y' && km.Mod == tea.ModCtrl

	if m.postUndo {
		if isRedo {
			want := m.contentBeforeUndo
			m.postUndo = false
			if next.Content != want {
				return []invariant.Violation{{
					InvariantID: "DL3",
					Message: fmt.Sprintf(
						"undo→redo did not restore Content: want %q got %q",
						trunc(want, 60), trunc(next.Content, 60),
					),
				}}
			}
			return nil
		}
		if !isUndo {
			m.postUndo = false // something else; cancel the pending check
		}
		return nil
	}

	// Trigger: undo that actually changes content.
	if isUndo && next.Content != prev.Content {
		m.postUndo = true
		m.contentBeforeUndo = prev.Content
	}
	return nil
}

// ---- EXT-NOCLOBBER: external-change clobber guard monitor (§1.4.7) ----
// When RunHuman performs a KindExternalWrite on the active file, the next ⌘S
// MUST return FileSaveErrorMsg{Conflict:true} — never a successful FileSavedMsg
// that would silently overwrite the newer bytes.

type extNoClobberMonitor struct {
	// externalWrite is set when we see ActiveFileExternallyModified=true in the
	// snapshot. It is reset when the conflict is correctly surfaced or the
	// active file path changes (write was for a background file).
	externalWrite bool
	watchedPath   string
}

func (m *extNoClobberMonitor) Reset() {
	m.externalWrite = false
	m.watchedPath = ""
}

func (m *extNoClobberMonitor) Observe(prev snapshot.Snapshot, msg tea.Msg, next snapshot.Snapshot) []invariant.Violation {
	// Arm: driver annotated that the active file was externally written.
	if next.ActiveFileExternallyModified {
		m.externalWrite = true
		m.watchedPath = next.ActiveFilePath
	}

	// Disarm on path change (write was for a tab that is no longer active).
	if m.externalWrite && next.ActiveFilePath != m.watchedPath {
		m.externalWrite = false
		m.watchedPath = ""
		return nil
	}

	if !m.externalWrite {
		return nil
	}

	// EXT-NOCLOBBER: a successful FileSavedMsg must NOT arrive while an
	// external write is pending — the save should have been refused.
	if savedMsg, ok := msg.(pgworkspace.FileSavedMsg); ok {
		_ = savedMsg
		m.externalWrite = false
		return []invariant.Violation{{
			InvariantID: "EXT-NOCLOBBER",
			Message: fmt.Sprintf(
				"FileSavedMsg succeeded for %q after external write — expected FileSaveErrorMsg{Conflict:true}",
				trunc(next.ActiveFilePath, 80),
			),
		}}
	}

	// Conflict correctly surfaced — reset.
	if errMsg, ok := msg.(pgworkspace.FileSaveErrorMsg); ok && errMsg.Conflict {
		m.externalWrite = false
		m.watchedPath = ""
	}

	return nil
}
