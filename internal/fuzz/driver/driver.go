//go:build fuzzing

package driver

import (
	"errors"
	"reflect"

	tea "charm.land/bubbletea/v2"

	"rune/internal/fuzz/event"
	"rune/internal/fuzz/invariant"
	"rune/internal/fuzz/session"
	"rune/pkg/docstate"
	"rune/pkg/editor/buffer"
	"rune/pkg/ui/components/filetree"
	"rune/pkg/ui/components/textedit"
	pgworkspace "rune/pkg/ui/pages/workspace"
	"rune/pkg/vfs"
)

// cmdSliceType is used for reflection-based detection of sequenceMsg.
var cmdSliceType = reflect.TypeOf([]tea.Cmd(nil))

// asCmdSlice detects messages that are a []tea.Cmd under the hood.
// This catches both the exported tea.BatchMsg and the unexported sequenceMsg.
func asCmdSlice(msg tea.Msg) ([]tea.Cmd, bool) {
	if msg == nil {
		return nil, false
	}
	if batch, ok := msg.(tea.BatchMsg); ok {
		return []tea.Cmd(batch), true
	}
	rv := reflect.ValueOf(msg)
	if rv.IsValid() && rv.Type().ConvertibleTo(cmdSliceType) {
		return rv.Convert(cmdSliceType).Interface().([]tea.Cmd), true
	}
	return nil, false
}

// runState holds per-run mutable state shared by drainMsg/drainCmd.
type runState struct {
	store          *docstate.Store
	monitors       []invariant.Monitor
	frozenFrame    string
	frozenCells    [][]textedit.Cell
	mirror         string
	appliedBatches int
	// externalWrites: set of paths for which RunHuman called mem.WriteFile but
	// the workspace has not yet resolved the conflict. drainMsg annotates
	// snap.ActiveFileExternallyModified = true whenever snap.ActiveFilePath is
	// in this set, arming the EXT-NOCLOBBER monitor. A path is removed once
	// the save resolves (FileSavedMsg or FileSaveErrorMsg{Conflict:true}).
	// The set covers both the active file and background tabs — the original
	// single-string approach missed background tabs opened after the write.
	externalWrites map[string]struct{}

	// reorder, when true, makes drainCmd DEFER async file-load results
	// (FileLoadedMsg/FileLoadErrorMsg) into `deferred` instead of delivering them
	// inline. RunReorder then replays them in a fuzz-chosen order to exercise the
	// out-of-order load race that the in-order drain structurally cannot reach.
	reorder  bool
	deferred []tea.Msg
}

// isLoadResult reports whether msg is an async file-load result — the only
// message class RunReorder defers, since out-of-order delivery of these is the
// race under test.
func isLoadResult(msg tea.Msg) bool {
	switch msg.(type) {
	case pgworkspace.FileLoadedMsg, pgworkspace.FileLoadErrorMsg:
		return true
	}
	return false
}

func (rs *runState) updateMirror(docID int64) {
	if rs.store == nil || docID == 0 {
		return
	}
	batches, err := rs.store.AllEdits(docID, "main")
	if err != nil {
		return // fire-and-forget: mirror is diagnostic; a store read error degrades DL1 coverage, never loses data
	}
	if rs.appliedBatches >= len(batches) {
		return
	}
	rs.mirror = buffer.ReplayForward(rs.mirror, batches[rs.appliedBatches:])
	rs.appliedBatches = len(batches)
}

// drainMsg drives one message through Update, checks all invariants, then
// drains any returned Cmd recursively.
func drainMsg(rs *runState, m pgworkspace.Model, msg tea.Msg) (pgworkspace.Model, *invariant.Violation) {
	prev := m.FuzzInspect()
	m, cmd := m.Update(msg)
	snap := m.FuzzInspect()

	rs.updateMirror(snap.DocID)
	snap.Frame = m.View().Content

	if rs.mirror != "" || snap.Content != "" {
		snap.MirrorContent = rs.mirror
	}

	snap.CloseFileKeyPressed = m.IsCloseFileMsg(msg)

	if _, ok := msg.(tea.QuitMsg); ok {
		snap.AppQuitting = true
	}

	// EXT-NOCLOBBER annotation: if the active file has a pending external write,
	// flag the snapshot so the monitor arms. Use msg.Path (not snap.ActiveFilePath)
	// to clear, because in-flight saves can settle on a tab that is no longer active.
	if len(rs.externalWrites) > 0 {
		if _, pending := rs.externalWrites[snap.ActiveFilePath]; pending {
			snap.ActiveFileExternallyModified = true
		}
		if savedMsg, ok := msg.(pgworkspace.FileSavedMsg); ok {
			delete(rs.externalWrites, savedMsg.Path)
		}
		if errMsg, ok := msg.(pgworkspace.FileSaveErrorMsg); ok && errMsg.Conflict {
			delete(rs.externalWrites, errMsg.Path)
		}
	}

	if v := session.Check(snap); v != nil {
		rs.frozenFrame = snap.Frame
		rs.frozenCells = snap.Cells
		return m, v
	}
	if vs := session.CheckTransition(prev, msg, snap); len(vs) > 0 {
		rs.frozenFrame = snap.Frame
		rs.frozenCells = snap.Cells
		return m, &vs[0]
	}
	if vs := session.ObserveMonitors(rs.monitors, prev, msg, snap); len(vs) > 0 {
		rs.frozenFrame = snap.Frame
		rs.frozenCells = snap.Cells
		return m, &vs[0]
	}
	if _, ok := msg.(pgworkspace.AutosaveSettledMsg); ok {
		var vfsContent string
		if rs.store != nil && snap.DocID != 0 {
			vfsContent, _ = rs.store.Content(snap.DocID) // fire-and-forget: read error → empty vfsContent → DL1 skips; no data loss
		}
		if v := session.CheckDataLoss(snap, vfsContent); v != nil {
			rs.frozenFrame = snap.Frame
			rs.frozenCells = snap.Cells
			return m, v
		}
	}

	if cmd == nil {
		return m, nil
	}
	return drainCmd(rs, m, cmd)
}

func drainCmd(rs *runState, m pgworkspace.Model, cmd tea.Cmd) (pgworkspace.Model, *invariant.Violation) {
	msg := cmd()
	if msg == nil {
		return m, nil
	}
	if cmds, ok := asCmdSlice(msg); ok {
		for _, c := range cmds {
			if c == nil {
				continue
			}
			var v *invariant.Violation
			m, v = drainCmd(rs, m, c)
			if v != nil {
				return m, v
			}
		}
		return m, nil
	}
	if rs.reorder && isLoadResult(msg) {
		// Defer the read result; RunReorder replays it (and its siblings) in a
		// fuzz-chosen order. The read itself already ran (cmd() above), so the
		// fsys access happened — only the delivery to Update is deferred.
		rs.deferred = append(rs.deferred, msg)
		return m, nil
	}
	return drainMsg(rs, m, msg)
}

// bootstrap drives WindowSizeMsg → Init → StoreReadyMsg on a fresh model.
func bootstrap(rs *runState, model pgworkspace.Model, store *docstate.Store, w, h int) (pgworkspace.Model, *invariant.Violation) {
	var v *invariant.Violation
	model, v = drainMsg(rs, model, tea.WindowSizeMsg{Width: w, Height: h})
	if v != nil {
		return model, v
	}
	if initCmd := model.Init(); initCmd != nil {
		model, v = drainCmd(rs, model, initCmd)
		if v != nil {
			return model, v
		}
	}
	model, v = drainMsg(rs, model, pgworkspace.StoreReadyMsg{Store: store})
	return model, v
}

// Run bootstraps a workspace.Model with the given store, drives it through
// events, and returns the first invariant Violation found (or nil), the frozen
// frame string, and the cell grid snapshot at the moment of violation.
func Run(model pgworkspace.Model, events []event.Event, store *docstate.Store, w, h int) (*invariant.Violation, string, [][]textedit.Cell) {
	rs := &runState{store: store, monitors: session.NewMonitors()}

	var v *invariant.Violation
	model, v = bootstrap(rs, model, store, w, h)
	if v != nil {
		return v, rs.frozenFrame, rs.frozenCells
	}
	for _, ev := range events {
		msg := eventToMsg(ev)
		if msg == nil {
			continue
		}
		model, v = drainMsg(rs, model, msg)
		if v != nil {
			return v, rs.frozenFrame, rs.frozenCells
		}
	}
	return nil, "", nil
}

// RunReorder fuzzes the load-correlation guard AND its interaction with a
// synchronous superseding transition — the properties Run/RunHuman structurally
// cannot reach (they settle every Cmd inline, in issue order, so a load is never
// pending across another transition).
//
// Shape, all fuzz-chosen: optionally open `settle` and deliver its read INLINE so
// the displayed doc becomes a real file; then open each of `opens` with its read
// DEFERRED; then, if `supersede`, press Ctrl+N (CreateUntitled). Finally replay
// every deferred read in the permutation `order`. A driver-side mirror of the
// guard predicts the displayed doc; after replay the model MUST match it:
//
//   - supersede over a FILE view runs supersedeLoad, clearing the gate, so EVERY
//     pending deferred read is stale and dropped → the new untitled stays shown
//     (the gate-clearing transition the all-deferred path never exercises).
//   - otherwise the most-recent open's read wins regardless of delivery order; a
//     re-open of the currently shown file is a no-op (requestOpenPath early-returns).
func RunReorder(model pgworkspace.Model, settle string, opens []string, supersede bool, order []byte, store *docstate.Store, w, h int) (*invariant.Violation, string, [][]textedit.Cell) {
	rs := &runState{store: store, monitors: session.NewMonitors()}

	var v *invariant.Violation
	model, v = bootstrap(rs, model, store, w, h)
	if v != nil {
		return v, rs.frozenFrame, rs.frozenCells
	}

	// Mirror of the guard, enough to predict the displayed doc. display is the
	// shown path ("" = the bootstrap untitled; this harness never types, so
	// display=="" ⇔ untitled). pendActive/pendPath track the single still-pending
	// read that would win — the model keeps only the latest request's generation.
	display := ""
	pendActive := false
	pendPath := ""

	if settle != "" {
		rs.reorder = false // deliver this read inline so the view becomes a file
		model, v = drainMsg(rs, model, filetree.FileSelectedMsg{Path: settle})
		if v != nil {
			return v, rs.frozenFrame, rs.frozenCells
		}
		display = settle
	}

	rs.reorder = true // from here, file-load results are deferred
	for _, p := range opens {
		if p == "" || p == display {
			continue // re-open of the shown file issues no new read (early return)
		}
		model, v = drainMsg(rs, model, filetree.FileSelectedMsg{Path: p})
		if v != nil {
			return v, rs.frozenFrame, rs.frozenCells
		}
		pendActive = true
		pendPath = p // display unchanged: the read is deferred
	}

	if supersede {
		rs.reorder = false // Ctrl+N emits only title/snapshot cmds — deliver inline
		model, v = drainMsg(rs, model, tea.KeyPressMsg{Code: 'n', Mod: tea.ModCtrl})
		if v != nil {
			return v, rs.frozenFrame, rs.frozenCells
		}
		if display != "" {
			// View was a file → CreateUntitled ran supersedeLoad: the gate is
			// cleared, so every pending deferred read is now stale.
			display = ""
			pendActive = false
		}
		// else already untitled → Ctrl+N no-ops (guard); the pending read stands.
	}

	// Replay the deferred reads in the fuzz-chosen order; every invariant runs on
	// each delivery (so the whole suite is exercised under reordering, not just
	// the final property).
	oi := 0
	for len(rs.deferred) > 0 {
		pick := 0
		if oi < len(order) {
			pick = int(order[oi]) % len(rs.deferred)
		}
		oi++
		msg := rs.deferred[pick]
		rs.deferred = append(rs.deferred[:pick:pick], rs.deferred[pick+1:]...)
		model, v = drainMsg(rs, model, msg)
		if v != nil {
			return v, rs.frozenFrame, rs.frozenCells
		}
	}

	// Predicted vs actual displayed doc.
	want := display
	if pendActive {
		want = pendPath
	}
	if snap := model.FuzzInspect(); snap.ActiveFilePath != want {
		return &invariant.Violation{
			InvariantID: "LOAD-LASTWINS",
			Message: "displayed " + invariant.Trunc(orUntitled(snap.ActiveFilePath), 80) +
				", want last-transition " + invariant.Trunc(orUntitled(want), 80),
		}, model.View().Content, nil
	}
	return nil, "", nil
}

// orUntitled renders the empty (untitled) path readably in a violation message.
func orUntitled(p string) string {
	if p == "" {
		return "<untitled>"
	}
	return p
}

// RunHuman is the entry point for FuzzHumanSession. It extends Run with
// support for KindExternalWrite and KindWatch events, which require access to
// the shared vfs.Mem and the list of seeded file paths.
//
//   - KindExternalWrite: calls mem.WriteFile for paths[ev.PathIndex % len(paths)].
//     Advances Mem's mod-clock so the workspace's diskBaseline becomes stale;
//     the next ⌘S must surface FileSaveErrorMsg{Conflict:true} (§1.4.7).
//     No model message is injected. Adds the path to rs.externalWrites so the
//     EXT-NOCLOBBER monitor is armed both for the active file and background tabs.
//
//   - KindWatch(sub=0): injects workspace.FuzzDirChangedMsg() → workspace calls
//     reloadDirCmd → drains to filetree.DirReloadedMsg.
//
//   - KindWatch(sub=1): injects workspace.FuzzFileWatchReadErrorMsg() so the
//     read-error path is exercised.
func RunHuman(model pgworkspace.Model, events []event.Event, store *docstate.Store, mem *vfs.Mem, paths []string, w, h int) (*invariant.Violation, string, [][]textedit.Cell) {
	rs := &runState{store: store, monitors: session.NewMonitors()}

	var v *invariant.Violation
	model, v = bootstrap(rs, model, store, w, h)
	if v != nil {
		return v, rs.frozenFrame, rs.frozenCells
	}

	for _, ev := range events {
		switch ev.Kind {
		case event.KindExternalWrite:
			if mem == nil || len(paths) == 0 {
				continue
			}
			path := paths[int(ev.PathIndex)%len(paths)]
			content := ev.Text
			if content == "" {
				content = "external-write\n" // non-empty so baseline size diverges
			}
			_ = mem.WriteFile(path, []byte(content), 0o644) // fire-and-forget: Mem never fails
			// Arm EXT-NOCLOBBER for this path — covers both active and background tabs.
			if rs.externalWrites == nil {
				rs.externalWrites = make(map[string]struct{})
			}
			rs.externalWrites[path] = struct{}{}
			continue

		case event.KindWatch:
			var msg tea.Msg
			if ev.WatchSub == 0 {
				msg = pgworkspace.FuzzDirChangedMsg()
			} else {
				snap := model.FuzzInspect()
				msg = pgworkspace.FuzzFileWatchReadErrorMsg(snap.ActiveFilePath, errors.New("watch: simulated read error"))
			}
			model, v = drainMsg(rs, model, msg)

		default:
			msg := eventToMsg(ev)
			if msg == nil {
				continue
			}
			model, v = drainMsg(rs, model, msg)
		}

		if v != nil {
			return v, rs.frozenFrame, rs.frozenCells
		}
	}
	return nil, "", nil
}
