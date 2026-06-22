//go:build fuzzing

package driver

import (
	"reflect"
	"sort"

	tea "charm.land/bubbletea/v2"

	"rune/internal/fuzz/event"
	"rune/internal/fuzz/invariant"
	"rune/pkg/docstate"
	"rune/pkg/editor/buffer"
	"rune/pkg/ui/components/textedit"
	"rune/pkg/ui/pages/workspace"
)

// cmdSliceType is used for reflection-based detection of sequenceMsg.
var cmdSliceType = reflect.TypeOf([]tea.Cmd(nil))

// asCmdSlice detects messages that are a []tea.Cmd under the hood.
// This catches both the exported tea.BatchMsg and the unexported sequenceMsg.
func asCmdSlice(msg tea.Msg) ([]tea.Cmd, bool) {
	if msg == nil {
		return nil, false
	}
	// Fast path: exported BatchMsg
	if batch, ok := msg.(tea.BatchMsg); ok {
		return []tea.Cmd(batch), true
	}
	// Reflection: catch unexported sequenceMsg (underlying type []tea.Cmd)
	rv := reflect.ValueOf(msg)
	if rv.IsValid() && rv.Type().ConvertibleTo(cmdSliceType) {
		return rv.Convert(cmdSliceType).Interface().([]tea.Cmd), true
	}
	return nil, false
}

// replayBatch applies one edit batch to the mirror string.
// Each AppliedEdit.Start is a post-shift new-buffer offset.
// Applying batches in ascending-Start order makes each edit's baked-in
// shift align with the running mirror displacement — correct for multi-cursor.
func replayBatch(mirror string, batch []buffer.AppliedEdit) string {
	// Sort ascending by Start (new-buffer offset).
	sorted := make([]buffer.AppliedEdit, len(batch))
	copy(sorted, batch)
	sort.Slice(sorted, func(i, j int) bool {
		return sorted[i].Start < sorted[j].Start
	})
	for _, e := range sorted {
		if e.Start < 0 || e.Start > len(mirror) {
			continue // guard against corrupt data
		}
		skip := len(e.Deleted)
		tail := e.Start + skip
		if tail > len(mirror) {
			tail = len(mirror)
		}
		mirror = mirror[:e.Start] + e.Insert + mirror[tail:]
	}
	return mirror
}

// Run bootstraps a workspace.Model with the given store, drives it through events,
// and returns the first invariant Violation found (or nil), the frozen frame string,
// and the cell grid snapshot at the moment of violation.
//
// Bootstrap sequence:
//  1. WindowSizeMsg{w, h} → drain
//  2. model.Init() → drain
//  3. StoreReadyMsg{Store: store} → drain
//  4. Feed events one by one → drain after each
//
// CheckInvariants is called after every settled message (after full cmd drain).
// CheckTransition is called with (prev, msg, next) for every drainMsg invocation.
func Run(model workspace.Model, events []event.Event, store *docstate.Store, w, h int) (*invariant.Violation, string, [][]textedit.Cell) {
	frozenFrame := ""
	var frozenCells [][]textedit.Cell

	// SHADOW mirror: maintained incrementally by replaying journal batches.
	// Pinned to a single docID so the one-continuous-"main"-journal assumption holds (N8).
	mirror := ""
	appliedBatches := 0 // number of journal batches already replayed into mirror

	// monitors is the set of stateful L2 monitors reset per Run call.
	monitors := invariant.NewMonitors()

	updateMirror := func(docID int64) {
		if store == nil || docID == 0 {
			return
		}
		batches, err := store.AllEdits(docID, "main")
		if err != nil {
			return
		}
		for i := appliedBatches; i < len(batches); i++ {
			mirror = replayBatch(mirror, batches[i])
		}
		appliedBatches = len(batches)
	}

	var drainMsg func(m workspace.Model, msg tea.Msg) (workspace.Model, *invariant.Violation)
	var drainCmd func(m workspace.Model, cmd tea.Cmd) (workspace.Model, *invariant.Violation)

	drainMsg = func(m workspace.Model, msg tea.Msg) (workspace.Model, *invariant.Violation) {
		prev := m.FuzzInspect()
		m, cmd := m.Update(msg)

		snap := m.FuzzInspect()

		// Update SHADOW mirror from new journal entries for the active doc (N8: pinned to snap.DocID).
		updateMirror(snap.DocID)
		snap.Frame = m.View().Content

		// Wire SHADOW: set mirror content for comparison.
		if mirror != "" || snap.Content != "" {
			snap.MirrorContent = mirror
		}

		// G3 annotation: flag CloseFile key presses so CheckTransition can assert the guard.
		snap.CloseFileKeyPressed = m.IsCloseFileMsg(msg)

		// Propagate AppQuitting marker set by driver on tea.QuitMsg.
		if _, ok := msg.(tea.QuitMsg); ok {
			snap.AppQuitting = true
		}

		// L0 invariants.
		if v := invariant.CheckInvariants(snap); v != nil {
			frozenFrame = snap.Frame
			frozenCells = snap.Cells
			return m, v
		}

		// L1 transition invariants.
		if vs := invariant.CheckTransition(prev, msg, snap); len(vs) > 0 {
			frozenFrame = snap.Frame
			frozenCells = snap.Cells
			return m, &vs[0]
		}

		// L2 monitor invariants.
		if vs := invariant.ObserveMonitors(monitors, prev, msg, snap); len(vs) > 0 {
			frozenFrame = snap.Frame
			frozenCells = snap.Cells
			return m, &vs[0]
		}

		// DL1: VFS content must equal buffer immediately after an autosave snapshot settles.
		// The driver reads VFS content and passes it to keep invariant docstate-free (N2).
		if _, ok := msg.(workspace.AutosaveSettledMsg); ok {
			var vfsContent string
			if store != nil && snap.DocID != 0 {
				vfsContent, _ = store.Content(snap.DocID) // reconstructs at current_seq
			}
			if v := invariant.CheckDataLossInvariants(snap, vfsContent); v != nil {
				frozenFrame = snap.Frame
				frozenCells = snap.Cells
				return m, v
			}
		}

		if cmd == nil {
			return m, nil
		}
		return drainCmd(m, cmd)
	}

	drainCmd = func(m workspace.Model, cmd tea.Cmd) (workspace.Model, *invariant.Violation) {
		msg := cmd()
		if msg == nil {
			return m, nil
		}
		// Detect BatchMsg and sequenceMsg (both []tea.Cmd underneath)
		if cmds, ok := asCmdSlice(msg); ok {
			for _, c := range cmds {
				if c == nil {
					continue
				}
				var v *invariant.Violation
				m, v = drainCmd(m, c)
				if v != nil {
					return m, v
				}
			}
			return m, nil
		}
		return drainMsg(m, msg)
	}

	// Step 1: WindowSizeMsg
	var v *invariant.Violation
	model, v = drainMsg(model, tea.WindowSizeMsg{Width: w, Height: h})
	if v != nil {
		return v, frozenFrame, frozenCells
	}

	// Step 2: Init()
	initCmd := model.Init()
	if initCmd != nil {
		model, v = drainCmd(model, initCmd)
		if v != nil {
			return v, frozenFrame, frozenCells
		}
	}

	// Step 3: Inject store
	model, v = drainMsg(model, workspace.StoreReadyMsg{Store: store})
	if v != nil {
		return v, frozenFrame, frozenCells
	}

	// Step 4: Drive events
	for _, ev := range events {
		msg := eventToMsg(ev)
		if msg == nil {
			continue
		}
		model, v = drainMsg(model, msg)
		if v != nil {
			return v, frozenFrame, frozenCells
		}
	}

	return nil, "", nil
}

// trunc truncates s to at most n bytes.
func trunc(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n] + "…"
}
