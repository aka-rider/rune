//go:build fuzzing

package driver

import (
	"errors"
	"fmt"
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
	store       *docstate.Store
	monitors    []invariant.Monitor
	frozenFrame string
	frozenCells [][]textedit.Cell
	// baselines maps a docID to the content it was LOADED with — captured the first
	// time the doc is observed with zero journaled edits (when its buffer IS the
	// freshly-loaded content). The SHADOW mirror for a doc is baseline +
	// ReplayForward(all "main" edits): an independent reconstruction of the live
	// buffer that, unlike a single global replay-from-empty, correctly handles a
	// non-empty loaded baseline and multiple documents.
	baselines map[int64]string
	// externalWrites: set of paths for which RunHuman called mem.WriteFile but the
	// workspace has not yet reconciled the conflict. drainMsg checks EXT-NOCLOBBER
	// directly against this set (the driver authoritatively owns it): a FileSavedMsg
	// for a path still in the set means production clobbered newer bytes instead of
	// refusing. A path is removed once reconciled — a refused save
	// (FileSaveErrorMsg{Conflict:true}) or a reload (FileLoadedMsg) that refreshes
	// production's diskBaseline. The set covers both the active file and background
	// tabs — the original single-string approach missed background tabs opened after
	// the write.
	externalWrites map[string]struct{}

	// reorder, when true, makes drainCmd DEFER async file-load results
	// (FileLoadedMsg/FileLoadErrorMsg) into `deferred` instead of delivering them
	// inline. RunReorder then replays them in a fuzz-chosen order to exercise the
	// out-of-order load race that the in-order drain structurally cannot reach.
	reorder  bool
	deferred []tea.Msg

	// reorderSaves, when true, makes drainCmd DEFER async SAVE results
	// (FileSavedMsg/FileSaveErrorMsg) into `deferredSaves` instead of delivering
	// them inline, so subsequent fuzz events (edits) are journaled while the save
	// is "in flight". RunReorderSaves replays the deferred save afterward; the
	// SAVE-RACE invariant then asserts the saved doc is correctly STILL dirty —
	// the edit-during-save interleaving that exposes the MarkSaved seq race
	// (§1.4.2/§1.4.8), which the inline drain structurally cannot reach.
	reorderSaves  bool
	deferredSaves []deferredSave
}

// deferredSave records an async save result whose delivery to Update is delayed,
// together with the journal position captured when it was issued, so the SAVE-RACE
// invariant can tell whether edits were journaled while the save was in flight.
type deferredSave struct {
	msg      tea.Msg
	docID    int64
	issueSeq int64 // store.CurrentSeq(docID) at the moment the save was issued
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

// mirrorFor reconstructs the document's content independently of the live editor
// buffer: the loaded baseline plus every journaled "main" edit replayed forward.
// The baseline is captured the first time a doc is seen with zero edits (its buffer
// IS the freshly-loaded content then); later edits replay on top — matching how the
// editor builds the buffer (loaded content + edits). This lets SHADOW validate that
// the journal faithfully tracks the buffer for LOADED files, not just untitled ones
// born empty. AllEdits returns the full edit log (not just post-snapshot), so the
// reconstruction is correct across autosave snapshots.
func (rs *runState) mirrorFor(docID int64, bufferContent string) string {
	if rs.store == nil || docID == 0 {
		return ""
	}
	// ActiveEdits (not AllEdits) so the mirror honors the undo head (seq <=
	// current_seq): after an undo the buffer drops the undone edits, and the mirror
	// must too, or it diverges (SHADOW).
	batches, err := rs.store.ActiveEdits(docID, "main")
	if err != nil {
		return rs.baselines[docID] // fire-and-forget: store read error degrades SHADOW coverage, never loses data
	}
	if len(batches) == 0 {
		// No live edits (fresh load, or everything undone) → the buffer is exactly the
		// loaded baseline; (re)record it per-doc so subsequent edits replay on top.
		rs.baselines[docID] = bufferContent
		return bufferContent
	}
	return buffer.ReplayForward(rs.baselines[docID], batches)
}

// drainMsg drives one message through Update, checks all invariants, then
// drains any returned Cmd recursively.
func drainMsg(rs *runState, m pgworkspace.Model, msg tea.Msg) (pgworkspace.Model, *invariant.Violation) {
	prev := m.FuzzInspect()
	m, cmd := m.Update(msg)
	snap := m.FuzzInspect()

	mirror := rs.mirrorFor(snap.DocID, snap.Content)
	snap.Frame = m.View().Content

	if mirror != "" || snap.Content != "" {
		snap.MirrorContent = mirror
	}

	snap.CloseFileKeyPressed = m.IsCloseFileMsg(msg)

	if _, ok := msg.(tea.QuitMsg); ok {
		snap.AppQuitting = true
	}

	// EXT-NOCLOBBER (§1.4.7): the driver authoritatively owns the set of paths with
	// an unreconciled external write (it performed them), so the check lives here
	// rather than in a monitor re-deriving it from a snapshot flag. A FileSavedMsg
	// for such a path means production overwrote newer bytes instead of refusing. A
	// refused save (FileSaveErrorMsg{Conflict}) OR a reload (FileLoadedMsg, which
	// refreshes production's diskBaseline) reconciles the obligation.
	if len(rs.externalWrites) > 0 {
		switch dm := msg.(type) {
		case pgworkspace.FileSavedMsg:
			if _, pending := rs.externalWrites[dm.Path]; pending {
				rs.frozenFrame = snap.Frame
				rs.frozenCells = snap.Cells
				return m, &invariant.Violation{
					InvariantID: "EXT-NOCLOBBER",
					Message: "FileSavedMsg succeeded for " + invariant.Trunc(dm.Path, 80) +
						" after external write — expected FileSaveErrorMsg{Conflict:true}",
				}
			}
		case pgworkspace.FileSaveErrorMsg:
			if dm.Conflict {
				delete(rs.externalWrites, dm.Path)
			}
		case pgworkspace.FileLoadedMsg:
			delete(rs.externalWrites, dm.Path)
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

	// TR-dirty-clear (§1.4.8): after a save settles, the SAVED doc must be clean.
	// Dirtiness is derived from the store (saved_seq vs current_seq), keyed to the
	// saved doc, so a still-dirty *other* tab no longer trips a false positive. Skip
	// in save-deferral mode, where edits are deliberately interleaved and the
	// SAVE-RACE invariant owns the (correctly-dirty) outcome.
	if savedMsg, ok := msg.(pgworkspace.FileSavedMsg); ok && !rs.reorderSaves && rs.store != nil && savedMsg.DocID != 0 {
		if dirty, err := rs.store.IsDirty(savedMsg.DocID); err == nil {
			if v := session.CheckSaveDirty(dirty); v != nil {
				rs.frozenFrame = snap.Frame
				rs.frozenCells = snap.Cells
				return m, v
			}
		}
	}

	if _, ok := msg.(pgworkspace.AutosaveSettledMsg); ok && rs.store != nil && snap.DocID != 0 {
		// A store read error means we cannot observe the durable VFS state — skip DL1
		// rather than fire a false positive. An empty but *successful* read is a valid
		// empty document (RecoverDocument reconstructs "" faithfully).
		if vfsContent, err := rs.store.Content(snap.DocID); err == nil {
			if v := session.CheckDataLoss(snap, vfsContent); v != nil {
				rs.frozenFrame = snap.Frame
				rs.frozenCells = snap.Cells
				return m, v
			}
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
	if rs.reorderSaves {
		if savedMsg, ok := msg.(pgworkspace.FileSavedMsg); ok && rs.store != nil && savedMsg.DocID != 0 {
			// Capture the journal position the saved bytes reflect — the save Cmd
			// ran inline immediately after ⌘S, so no edits have advanced the head
			// yet — then DEFER delivery so RunReorderSaves can journal more edits
			// before MarkSaved runs, exposing the seq race (§1.4.2/§1.4.8).
			issueSeq, _ := rs.store.CurrentSeq(savedMsg.DocID) // fire-and-forget: read error → issueSeq 0, SAVE-RACE still conservative
			rs.deferredSaves = append(rs.deferredSaves, deferredSave{msg: msg, docID: savedMsg.DocID, issueSeq: issueSeq})
			return m, nil
		}
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
	rs := &runState{store: store, monitors: session.NewMonitors(), baselines: map[int64]string{}}

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
	rs := &runState{store: store, monitors: session.NewMonitors(), baselines: map[int64]string{}}

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

// RunReorderSaves exercises the edit-during-save race that the in-order drain
// structurally cannot reach: it DEFERS each async save result (FileSavedMsg) so
// subsequent fuzz events keep journaling edits while the save is "in flight", then
// delivers the deferred saves and asserts SAVE-RACE — if edits were journaled after
// a save's issue position, the saved doc MUST still be dirty after the save settles.
// A MarkSaved that stamps the live journal head (instead of the position the written
// bytes reflect) marks the doc clean despite unsaved edits — a §1.4.2/§1.4.8
// durability bug the inline fuzzer (which settles each save before the next event)
// never sees.
func RunReorderSaves(model pgworkspace.Model, events []event.Event, store *docstate.Store, mem *vfs.Mem, paths []string, w, h int) (*invariant.Violation, string, [][]textedit.Cell) {
	rs := &runState{store: store, monitors: session.NewMonitors(), baselines: map[int64]string{}, reorderSaves: true}

	var v *invariant.Violation
	model, v = bootstrap(rs, model, store, w, h)
	if v != nil {
		return v, rs.frozenFrame, rs.frozenCells
	}

	for _, ev := range events {
		switch ev.Kind {
		case event.KindExternalWrite:
			// Advance the Mem mod-clock (not the focus of this harness — EXT-NOCLOBBER
			// is covered elsewhere; here it only perturbs save divergence).
			if mem != nil && len(paths) > 0 {
				path := paths[int(ev.PathIndex)%len(paths)]
				content := ev.Text
				if content == "" {
					content = "external-write\n"
				}
				_ = mem.WriteFile(path, []byte(content), 0o644) // fire-and-forget: Mem never fails
			}
			continue
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

	// Deliver each deferred save (running MarkSaved) AFTER the interleaved edits, then
	// assert the durability invariant. reorderSaves stays true so the inline
	// TR-dirty-clear check (which expects a clean doc) is skipped here — SAVE-RACE
	// owns the correctly-dirty outcome.
	for _, ds := range rs.deferredSaves {
		model, v = drainMsg(rs, model, ds.msg)
		if v != nil {
			return v, rs.frozenFrame, rs.frozenCells
		}
		if rs.store == nil || ds.docID == 0 {
			continue
		}
		cur, cerr := rs.store.CurrentSeq(ds.docID)
		if cerr != nil || cur <= ds.issueSeq {
			continue // no edits interleaved while the save was in flight → nothing to assert
		}
		dirty, derr := rs.store.IsDirty(ds.docID)
		if derr != nil {
			continue
		}
		if !dirty {
			snap := model.FuzzInspect()
			return &invariant.Violation{
				InvariantID: "SAVE-RACE",
				Message: fmt.Sprintf(
					"doc %d marked clean after save settled, but %d edit(s) were journaled while the save was in flight (saved@seq=%d, head=%d) — unsaved edits silently lost (§1.4.2)",
					ds.docID, cur-ds.issueSeq, ds.issueSeq, cur,
				),
			}, snap.Frame, snap.Cells
		}
	}
	return nil, "", nil
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
	rs := &runState{store: store, monitors: session.NewMonitors(), baselines: map[int64]string{}}

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
