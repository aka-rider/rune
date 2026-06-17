//go:build fuzzing

package invariant

import (
	"fmt"

	tea "charm.land/bubbletea/v2"
)

// Monitor is a stateful L2 invariant automaton. Observe is called after every
// settled message with (prev, msg, next). Reset is called once per shrink replay
// to clear automaton state so replays are deterministic.
type Monitor interface {
	Observe(prev Snapshot, msg tea.Msg, next Snapshot) []Violation
	Reset()
}

// monitorSet groups all active monitors.
type monitorSet []Monitor

// NewMonitors creates a fresh set of all L2 monitors for one Run call.
func NewMonitors() monitorSet {
	return monitorSet{
		&quitLivenessMonitor{},
		&undoRedoMonitor{},
	}
}

// ObserveMonitors fans out to all monitors and collects violations.
// Returns the first violation found (matches driver's first-wins behavior).
func ObserveMonitors(ms monitorSet, prev Snapshot, msg tea.Msg, next Snapshot) []Violation {
	for _, mon := range ms {
		if vs := mon.Observe(prev, msg, next); len(vs) > 0 {
			return vs
		}
	}
	return nil
}

// ---- F1: quit liveness monitor ----
// After footer.ConfirmQuitMsg fires without a guard blocking it, AppQuitting must
// become true within N settled steps.

const quitMaxSteps = 20

type quitLivenessMonitor struct {
	pending bool
	steps   int
}

func (m *quitLivenessMonitor) Reset() { m.pending = false; m.steps = 0 }

func (m *quitLivenessMonitor) Observe(_ Snapshot, msg tea.Msg, next Snapshot) []Violation {
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
			return []Violation{{
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

func (m *undoRedoMonitor) Observe(prev Snapshot, msg tea.Msg, next Snapshot) []Violation {
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
				return []Violation{{
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
