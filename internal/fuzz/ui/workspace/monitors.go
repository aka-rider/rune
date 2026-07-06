//go:build fuzzing

package workspace

import (
	"fmt"

	tea "charm.land/bubbletea/v2"

	"rune/internal/fuzz/invariant"
	"rune/internal/fuzz/snapshot"
	"rune/pkg/ui/components/footer"
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
		&unwindRedetectMonitor{},
		&quitSaveAllMonitor{},
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
	armed  bool // first click has focused paneTree at m.firstY
	firstY int
	cursor int // FiletreeCursor after the armed click
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

// ---- UNWIND-REDETECT (L2): merge deactivation via undo/redo must settle ----
// resyncMergeIfMain (workspace_merge_fresh.go) runs on EVERY undo/redo
// journal jump on "main" (workspace_undo.go calls it from both the undo AND
// the redo handler); when it takes the resolver active→inactive, it
// launches probeUnwindCmd — an async re-probe that either re-raises
// GuardMerge (still diverged) or leaves the doc legitimately settled. This
// is the LIVENESS half of that contract: the driver-level UNWIND-REDETECT
// (driver_verbatim.go) checks the unwindProbeMsg's OWN handling once it
// arrives; this catches the probe never even landing within budget.
// Disarmed by Escape: mergemode.Abort deactivates the resolver directly,
// with NO probe involved — a legitimate, different deactivation path this
// monitor does not police (see paneCenter's Cancel branch,
// workspace_update_keys.go).
const unwindRedetectMaxSteps = 20

type unwindRedetectMonitor struct {
	pending bool
	steps   int
}

func (m *unwindRedetectMonitor) Reset() { m.pending = false; m.steps = 0 }

// isUndoRedoKey mirrors keymap.Default()'s fixed Undo/Redo bindings
// (super+z/ctrl+z, shift+super+z/ctrl+y) — this leaf checker package stays
// decoupled from pkg/ui/keymap (see the %T-name + reflect convention used
// throughout internal/fuzz/ui/*), so the combos are hard-coded here rather
// than imported; keymap.Default() is fixed input across the whole fuzzer.
func isUndoRedoKey(msg tea.Msg) bool {
	kp, ok := msg.(tea.KeyPressMsg)
	if !ok {
		return false
	}
	if kp.Code == 'z' && (kp.Mod == tea.ModSuper || kp.Mod == tea.ModCtrl || kp.Mod == (tea.ModShift|tea.ModSuper)) {
		return true
	}
	return kp.Code == 'y' && kp.Mod == tea.ModCtrl
}

func (m *unwindRedetectMonitor) Observe(prev snapshot.Snapshot, msg tea.Msg, next snapshot.Snapshot) []invariant.Violation {
	// Disarm: Escape aborts the resolver directly (mergemode.Abort) — a
	// legitimate deactivation with no probe involved.
	if kp, ok := msg.(tea.KeyPressMsg); ok && kp.Code == tea.KeyEscape && kp.Mod == 0 {
		m.pending = false
		m.steps = 0
		return nil
	}

	if m.pending {
		// Satisfied: the probe landed (regardless of its outcome — a Clean
		// result is a legitimate settle, not a guard re-raise), the guard
		// reappeared, or a redo re-entered the resolver.
		typeName := fmt.Sprintf("%T", msg)
		if typeName == "workspace.unwindProbeMsg" || next.GuardVisible || next.MergeActive {
			m.pending = false
			m.steps = 0
			return nil
		}
		m.steps++
		if m.steps > unwindRedetectMaxSteps {
			m.pending = false
			return []invariant.Violation{{
				InvariantID: "UNWIND-REDETECT",
				Message: fmt.Sprintf(
					"merge deactivation via undo/redo not re-detected within %d steps (no unwindProbeMsg, guard, or re-entered resolver)",
					unwindRedetectMaxSteps,
				),
			}}
		}
		return nil
	}

	// Arm: an undo/redo keypress deactivates an ACTIVE resolver.
	if prev.MergeActive && !next.MergeActive && isUndoRedoKey(msg) {
		m.pending = true
		m.steps = 0
	}
	return nil
}

// ---- QUIT-SAVEALL (L2): a Save response to the quit-batch guard must reach AppQuitting ----
// Arms on footer.DataLossGuardResponseMsg{Save} while prev.PendingDataLossKind
// was Quit (pendingKindQuit) — handleDataLossGuardResponse's
// footer.DataLossSave branch calls saveAllDirtyForQuit for exactly that
// state. Disarms whenever next.PendingDataLossKind is no longer Quit: this is
// the general "the quit-batch was abandoned" signal, not just an async
// FileSaveErrorMsg (the original, too-narrow disarm — WP7 follow-up
// session). saveAllDirtyForQuit (workspace_quit.go) has FOUR early-return
// abort paths that clear pendingDataLoss SYNCHRONOUSLY, in the very same
// Update call that processes the [S]ave response, without ever issuing an
// async save or emitting FileSaveErrorMsg: a §1.4.8 re-derived
// SyncDiverged doc (abortQuitForDivergence), a failed divergence re-check, a
// missing CAS baseline, or a VFS-reconstruction failure for a background
// dirty tab. Each is a CORRECT, safety-critical refusal (§0/§1.4.7 — never
// silently clobber a diverged doc even mid-quit), not a liveness failure —
// found via FuzzHumanSession false-positiving on exactly this path (a
// repeated quitSaveAll cluster whose second attempt re-derived
// SyncDiverged for the sole dirty doc and correctly aborted the quit,
// resuming normal editing, with no FileSaveErrorMsg ever in the picture).
const quitSaveAllMaxSteps = 32

type quitSaveAllMonitor struct {
	pending bool
	steps   int
}

func (m *quitSaveAllMonitor) Reset() { m.pending = false; m.steps = 0 }

func (m *quitSaveAllMonitor) Observe(prev snapshot.Snapshot, msg tea.Msg, next snapshot.Snapshot) []invariant.Violation {
	if m.pending {
		if next.AppQuitting || next.PendingDataLossKind != pendingKindQuit {
			m.pending = false
			m.steps = 0
			return nil
		}
		m.steps++
		if m.steps > quitSaveAllMaxSteps {
			m.pending = false
			return []invariant.Violation{{
				InvariantID: "QUIT-SAVEALL",
				Message: fmt.Sprintf(
					"AppQuitting not reached within %d steps after a [S]ave response to the quit-batch guard",
					quitSaveAllMaxSteps,
				),
			}}
		}
		return nil
	}

	if resp, ok := msg.(footer.DataLossGuardResponseMsg); ok && prev.PendingDataLossKind == pendingKindQuit {
		if resp.Response == footer.DataLossSave {
			m.pending = true
			m.steps = 0
		}
	}
	return nil
}
