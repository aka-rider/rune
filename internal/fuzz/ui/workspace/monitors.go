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
		&filetreeClickReachMonitor{},
	}
}

// ---- FT-CLICK-REACH: filetree click must reach the tree ----
// The filetree self-gates on m.focused before acting on a click. A mouse click
// that focuses paneTree must also forward the click to the filetree in the same
// Update — not rely on the next click to "catch up".
//
// Detection: two consecutive left clicks at the same Y landing on paneTree.
// The first click must have moved the cursor to the clicked row; the second
// click on the same row must leave the cursor unchanged (it emits FileSelectedMsg
// instead). If the cursor moves on the second click, the first click did not
// reach the tree — the dispatch regression.
//
// paneTreeFuzz matches the workspace pane enum: 0=tree,1=tabs,2=center,3=title,4=chat,5=search.
const paneTreeFuzz = 0

type filetreeClickReachMonitor struct {
	armed   bool // first click has focused paneTree at m.firstY
	firstY  int
	cursor  int // FiletreeCursor after the armed click
}

func (m *filetreeClickReachMonitor) Reset() { m.armed = false }

func (m *filetreeClickReachMonitor) Observe(_ snapshot.Snapshot, msg tea.Msg, next snapshot.Snapshot) []invariant.Violation {
	click, ok := msg.(tea.MouseClickMsg)
	if !ok || click.Button != tea.MouseLeft {
		m.armed = false
		return nil
	}

	if m.armed && next.FocusPane == paneTreeFuzz && click.Y == m.firstY {
		// Second consecutive left click at the same Y while the tree is focused.
		// The cursor must not change: the first click already placed it at this row.
		if next.FiletreeCursor != m.cursor {
			v := invariant.Violation{
				InvariantID: "FT-CLICK-REACH",
				Message: fmt.Sprintf(
					"cursor moved %d→%d on second click at Y=%d: "+
						"first click did not reach the tree (dispatch regression)",
					m.cursor, next.FiletreeCursor, click.Y,
				),
			}
			m.armed = false
			return []invariant.Violation{v}
		}
		m.armed = false
		return nil
	}

	// Arm whenever a left click results in paneTree focus.
	if next.FocusPane == paneTreeFuzz {
		m.armed = true
		m.firstY = click.Y
		m.cursor = next.FiletreeCursor
	} else {
		m.armed = false
	}
	return nil
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
