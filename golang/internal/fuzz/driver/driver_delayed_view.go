package driver

import (
	"fmt"

	tea "charm.land/bubbletea/v2"

	"rune/internal/fuzz/invariant"
	"rune/internal/fuzz/session"
	"rune/pkg/docstate"
	"rune/pkg/ui/components/filetree"
	"rune/pkg/ui/components/textedit"
	pgworkspace "rune/pkg/ui/pages/workspace"
	"rune/pkg/vfs"
)

// isViewProbeResult reports whether msg is a view-targeted async probe
// result (Part IV I3) — resolveProbeMsg/unwindProbeMsg, both unexported
// inside pkg/ui/pages/workspace. Detected by type name (the same %T-based
// pattern internal/fuzz/ui/workspace already uses for unexported message
// types, e.g. "workspace.dirChangedMsg") since the driver cannot import an
// unexported type across the package boundary.
func isViewProbeResult(msg tea.Msg) bool {
	switch fmt.Sprintf("%T", msg) {
	case "workspace.resolveProbeMsg", "workspace.unwindProbeMsg":
		return true
	}
	return false
}

// RunDelayedViewResult exercises the Part IV viewTicket chokepoint under a
// DEFERRED [D]/[M] conflict resolution: load a file, change it externally (a
// real CAS conflict), attempt ⌘S (refused, raises the conflict guard),
// choose [D]iscard or [M]erge (fuzz-chosen) — deferring the resulting
// resolveProbeCmd's result instead of delivering it inline — THEN force an
// UNCONDITIONAL epoch bump (Ctrl+N, a fresh untitled — Part IV's bumpEpoch,
// also changing m.view.DocID() entirely, so the deferred result's ticket is
// stale by construction) BEFORE replaying it.
//
// Property (VIEW-TICKET-STALE): applying a view-targeted result whose ticket
// has gone stale must never mutate the buffer OR identity the user is now
// looking at. The in-order drain (Run/RunHuman) structurally cannot reach
// this: it settles every Cmd before the next event ever runs, so a
// view-targeted result is never still in flight when a later transition
// lands.
func RunDelayedViewResult(model pgworkspace.Model, path string, useMerge bool, store *docstate.Store, mem *vfs.Mem, w, h int) (*invariant.Violation, string, [][]textedit.Cell) {
	rs := &runState{store: store, monitors: session.NewMonitors(), baselines: map[int64]string{}, mem: mem}

	var v *invariant.Violation
	model, v = bootstrap(rs, model, store, w, h)
	if v != nil {
		return v, rs.frozenFrame, rs.frozenCells
	}

	const original = "shared\noriginal\n"
	const theirs = "shared\ntheirs changed\n"
	if err := mem.WriteFile(path, []byte(original), 0o644); err != nil {
		return nil, "", nil // fire-and-forget: Mem never fails in practice; skip run
	}

	model, v = drainMsg(rs, model, filetree.FileSelectedMsg{Path: path})
	if v != nil {
		return v, rs.frozenFrame, rs.frozenCells
	}
	if snap := model.FuzzInspect(); snap.DocID == 0 || snap.ActiveFilePath != path {
		return nil, "", nil // load didn't resolve identity (store race) — skip run, nothing to test
	}

	// A real local edit diverges ours from the ancestor Load just recorded —
	// needed for a genuine two-way conflict once theirs also diverges below
	// (an ancestor==ours reload would auto-resolve cleanly, never raising a
	// guard at all — see docstate's classifySync).
	model, v = drainMsg(rs, model, tea.KeyPressMsg{Code: 'X', Text: "X"})
	if v != nil {
		return v, rs.frozenFrame, rs.frozenCells
	}

	// An external write changes disk AFTER load — the save's CAS check will
	// refuse.
	if err := mem.WriteFile(path, []byte(theirs), 0o644); err != nil {
		return nil, "", nil
	}

	model, v = drainMsg(rs, model, tea.KeyPressMsg{Code: 's', Mod: tea.ModSuper})
	if v != nil {
		return v, rs.frozenFrame, rs.frozenCells
	}
	if !model.FuzzInspect().GuardVisible {
		return nil, "", nil // conflict wasn't reached this run (e.g. store race) — skip
	}

	// Real keypress ('d'/'m', matching guardMergeOptions in workspace.go) —
	// NOT footer.DataLossGuardResponseMsg constructed directly: in production
	// that message is only ever PRODUCED by the footer's own key handling
	// (handleKeyPress's InGuard priority branch), which is what clears the
	// footer's OWN guard state. Skipping straight to the response message
	// would leave GuardVisible stuck true (the G2 invariant catches exactly
	// this — it's a test-realism bug, not a production one).
	key := tea.KeyPressMsg{Code: 'd', Text: "d"}
	if useMerge {
		key = tea.KeyPressMsg{Code: 'm', Text: "m"}
	}
	rs.reorder = true // drainCmd also checks isViewProbeResult under this flag
	model, v = drainMsg(rs, model, key)
	if v != nil {
		return v, rs.frozenFrame, rs.frozenCells
	}
	if len(rs.deferred) == 0 {
		return nil, "", nil // resolution didn't launch a probe this run — skip
	}

	// Force an UNCONDITIONAL epoch bump + docID change (Ctrl+N) BEFORE
	// replaying — guarantees the deferred probe's ticket is now stale.
	rs.reorder = false
	model, v = drainMsg(rs, model, tea.KeyPressMsg{Code: 'n', Mod: tea.ModCtrl})
	if v != nil {
		return v, rs.frozenFrame, rs.frozenCells
	}
	before := model.FuzzInspect()

	for _, msg := range rs.deferred {
		model, v = drainMsg(rs, model, msg)
		if v != nil {
			return v, rs.frozenFrame, rs.frozenCells
		}
	}

	after := model.FuzzInspect()
	if after.Content != before.Content || after.DocID != before.DocID {
		return &invariant.Violation{
			InvariantID: "VIEW-TICKET-STALE",
			Message: fmt.Sprintf("a stale (cross-epoch) view-targeted result mutated the buffer: docID %d->%d, content %q->%q",
				before.DocID, after.DocID, invariant.Trunc(before.Content, 60), invariant.Trunc(after.Content, 60)),
		}, model.View().Content, nil
	}
	return nil, "", nil
}
