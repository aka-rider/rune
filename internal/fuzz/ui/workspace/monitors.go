//go:build fuzzing

package workspace

import (
	"fmt"

	tea "charm.land/bubbletea/v2"

	"rune/internal/fuzz/invariant"
	"rune/internal/fuzz/snapshot"
)

// NewMonitors returns a fresh set of all workspace L2 monitors for one Run call.
//
// Only the quit-liveness monitor (F1) remains: it tracks a bounded liveness
// counter, not a shadow of user data, so it cannot drift from production state.
// The former undoRedoMonitor (DL3) and extNoClobberMonitor were hand-maintained
// shadow state machines that re-derived production semantics and broke on N>1
// sequences. They are replaced by store-derived / driver-authoritative checks:
// DL3 is subsumed by SHADOW (buffer vs journal mirror via ActiveEdits, which
// already honors the undo head), and EXT-NOCLOBBER is checked in the driver
// directly against rs.externalWrites (the set the driver itself owns).
func NewMonitors() []invariant.Monitor {
	return []invariant.Monitor{
		&quitLivenessMonitor{},
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
