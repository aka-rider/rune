//go:build fuzzing

package driver

import (
	"reflect"

	tea "charm.land/bubbletea/v2"

	"rune/internal/fuzz/event"
	"rune/internal/fuzz/invariant"
	"rune/internal/fuzz/session"
	"rune/pkg/docstate"
	"rune/pkg/ui/components/filetree"
	"rune/pkg/ui/components/footer"
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

	// quit is set the instant a tea.QuitMsg is drained — see drainMsg's
	// comment. Every Run*/RunHuman event loop checks it and stops feeding
	// further script events once true (production has no "after quit"
	// state to keep exercising; the bubbletea runtime is already gone).
	quit bool

	// mem is the shared in-memory VFS every fuzz caller already constructs
	// (nil only for a caller that genuinely has none — checks using it are
	// skipped, never a false positive). Backs the disk↔buffer verbatim edge
	// of §1.4.5 (driver_verbatim.go): SAVE-VERBATIM/LOAD-VERBATIM compare
	// actual VFS bytes against the journal/buffer, which nothing before WP2
	// did (DL1/WP7(b) only pin store↔buffer).
	mem *vfs.Mem
	// baselines maps a docID to the content it was LOADED with — captured the first
	// time the doc is observed with zero journaled edits (when its buffer IS the
	// freshly-loaded content). The SHADOW mirror for a doc is baseline +
	// ReplayForward(all "main" edits): an independent reconstruction of the live
	// buffer that, unlike a single global replay-from-empty, correctly handles a
	// non-empty loaded baseline and multiple documents.
	baselines map[int64]string
	// mirrorCache caches mirrorFor's incremental reconstruction per docID —
	// see mirrorCacheEntry (driver_mirror.go) for the caching invariant.
	// Lazily initialized by setMirrorCache so the 5 runState{...} literals in
	// this package don't all need touching.
	mirrorCache map[int64]mirrorCacheEntry
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

	// steps counts settled messages, for checkV4Properties' deterministic
	// sampling of the expensive full-reconstruction property on very long
	// inputs (a full RecoverDocument per step is quadratic in journal
	// length — pathological ~300-op inputs took >5s per exec and tripped
	// the Go fuzz coordinator's worker-hang kill as flaky "hung or
	// terminated unexpectedly" failures).
	steps int

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

// drainMsg drives one message through Update, checks all invariants, then
// drains any returned Cmd recursively.
func drainMsg(rs *runState, m pgworkspace.Model, msg tea.Msg) (pgworkspace.Model, *invariant.Violation) {
	prev := m.FuzzInspect()
	m, cmd := m.Update(msg)
	snap := m.FuzzInspect()

	mirror := rs.mirrorFor(snap.DocID, snap.Content, isLoadFamilyMsg(msg, prev))
	snap.Frame = m.View().Content

	if mirror != "" || snap.Content != "" {
		snap.MirrorContent = mirror
	}

	snap.CloseFileKeyPressed = m.IsCloseFileMsg(msg)

	if _, ok := msg.(tea.QuitMsg); ok {
		snap.AppQuitting = true
		// The driver owns quit-liveness for every Run* loop: in production,
		// tea.Quit ends the bubbletea runtime — no further Msg is EVER
		// delivered to Update. The synchronous fuzz driver has no such
		// runtime to stop, so without this flag it keeps feeding subsequent
		// script events (KindWatch/KindExternalWrite/trash-confirm/etc.,
		// all reachable now that KindQuitRequest + quitSaveAll make a FULL
		// quit completable mid-run) into a torn-down Model whose store is
		// already closed (teardownAndQuit) — undefined territory production
		// never exercises. rs.quit lets every Run*/RunHuman loop stop
		// feeding events the instant that happens, mirroring reality.
		rs.quit = true
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
		// A THIRD reconciliation path: startSave's own pre-write §1.4.8 re-check
		// (workspace_edit.go) calls store.Sync synchronously BEFORE issuing any
		// Materialize Cmd, and raises GuardMerge directly — via the same
		// raiseConflictGuard chokepoint FileSaveErrorMsg{Conflict} and
		// FileLoadedMsg's load-time conflict use — the instant it sees
		// SyncDiverged. No FileSaveErrorMsg is ever emitted on that path, so the
		// two msg-type cases above never fire, yet the user has unambiguously
		// been shown the conflict (the same surfaced obligation FileSaveErrorMsg
		// reconciles) before any subsequent [S]ave-anyway can act. Reconciling
		// on "GuardMerge is now visible for the active path" — the one shared
		// discriminant every raiseConflictGuard caller produces — covers all
		// entry points without enumerating each one's message type.
		if snap.GuardVisible && snap.GuardKind == footer.GuardMerge {
			delete(rs.externalWrites, snap.ActiveFilePath)
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

	// WP7 properties (a)/(b)/(c) — extracted to driver_v4_properties.go to
	// keep this file under the 500-LoC limit (§1.6/§11).
	if v := checkV4Properties(rs, msg, snap); v != nil {
		rs.frozenFrame = snap.Frame
		rs.frozenCells = snap.Cells
		return m, v
	}

	// WP2 driver-level ground-truth checks (disk↔buffer verbatim edge of
	// §1.4.5) — extracted to driver_verbatim.go to keep this file under the
	// 500-LoC limit (§1.6/§11).
	if v := checkVerbatim(rs, m, msg, prev, snap); v != nil {
		rs.frozenFrame = snap.Frame
		rs.frozenCells = snap.Cells
		return m, v
	}

	// WP5 merge-lifecycle checks (DISCARD-ADOPT, MERGE-RESOLVE-ADOPT,
	// UNWIND-REDETECT(L1)) — driver_verbatim.go.
	if v := checkMergeLifecycle(rs, msg, snap); v != nil {
		rs.frozenFrame = snap.Frame
		rs.frozenCells = snap.Cells
		return m, v
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
	if rs.reorder && (isLoadResult(msg) || isViewProbeResult(msg)) {
		// Defer the read result; RunReorder replays load results (and their
		// siblings) in a fuzz-chosen order, RunDelayedViewResult replays a
		// deferred view-probe result after forcing the ticket stale
		// (driver_delayed_view.go, Part IV). The read itself already ran
		// (cmd() above), so the fsys access happened — only the delivery to
		// Update is deferred. isViewProbeResult never matches during
		// RunReorder's own scenarios (they never raise a conflict guard), so
		// this extension is a no-op there.
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
// mem is the same vfs.Mem the caller passed to workspace.New(...).WithFS —
// nil is accepted (verbatim checks skip); every current caller already holds one.
func Run(model pgworkspace.Model, events []event.Event, store *docstate.Store, mem *vfs.Mem, w, h int) (*invariant.Violation, string, [][]textedit.Cell) {
	rs := &runState{store: store, monitors: session.NewMonitors(), baselines: map[int64]string{}, mem: mem}

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
		if rs.quit {
			break // production has no "after quit" state to keep exercising
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
func RunReorder(model pgworkspace.Model, settle string, opens []string, supersede bool, order []byte, store *docstate.Store, mem *vfs.Mem, w, h int) (*invariant.Violation, string, [][]textedit.Cell) {
	rs := &runState{store: store, monitors: session.NewMonitors(), baselines: map[int64]string{}, mem: mem}

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
