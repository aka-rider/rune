package workspace

import (
	"errors"
	"io/fs"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/docstate"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/components/opentabs"
)

// savingTarget reports whether (docID, path) is the file OUR OWN interactive
// save is currently writing. Targeted — compares against the save's own
// identity captured at start (activeSave.Path/DocID), not just "some save is
// in flight" — a blanket activeSave.InFlight check would wrongly suppress a
// probe for an unrelated document merely because a different tab happens to
// be saving.
func (m Model) savingTarget(docID int64, path string) bool {
	if !m.activeSave.InFlight {
		return false
	}
	return opentabs.TabHandle{DocID: m.activeSave.DocID, Path: m.activeSave.Path}.
		Equal(opentabs.TabHandle{DocID: docID, Path: path})
}

// flushPendingReopen replays a navigation request that requestOpenPath
// deferred because it targeted the file an interactive save was writing
// (savingTarget). Called once that save settles — both the FileSavedMsg and
// FileSaveErrorMsg activeSave-matched branches — by which point
// activeSave.InFlight is already false, so the replayed requestOpenPath's
// savingTarget check can never re-defer and loop.
func (m Model) flushPendingReopen() (Model, tea.Cmd) {
	if !m.pendingReopen.active {
		return m, nil
	}
	reopen := m.pendingReopen
	m.pendingReopen = pendingReopen{}
	return m.requestOpenPath(reopen.docID, reopen.path)
}

// raiseDeletedGuard arms guard.deleted and raises the GuardDeleted footer
// prompt for the current document, whose file went missing on disk. Clears any
// stale top-banner m.err — the deletion flow uses the footer guard exclusively,
// never the sticky top error banner (the focus-trap root cause this guard
// replaces).
func (m Model) raiseDeletedGuard(docID int64, path string) Model {
	m.guard.deleted = deletedIntent{active: true, path: path, docID: docID}
	m.err = nil
	m = m.raiseGuardPrompt(guardDeleted)
	return m
}

// setDiskChangedHint derives the footer's persistent "changed on disk"
// indicator (Fix C / BUG1) from the same transition that sets
// m.diskChangedHint, so the two never drift apart (§1.4.8): every site that
// used to assign m.diskChangedHint directly now goes through this one helper.
func (m Model) setDiskChangedHint(changed bool) Model {
	m.diskChangedHint = changed
	m.footer = m.footer.SetDiskChanged(changed)
	return m
}

// ---- WP5: watcher/focus/flush handlers become Probe Cmds ------------------
//
// Tab-focus no longer needs its own probe at all: requestOpenPath always
// routes a file switch through store.Load (even for a previously-visited
// tab), and Load's own SyncState IS the freshness check — handleFileLoadedMsg
// derives the hint from msg.Result.Sync.Kind directly (see
// workspace_io_handlers.go), so there is no extra disk round trip for
// "stat-on-focus" (closing today's synchronous Stat in that path — §5.3).
//
// dirChangedMsg (fsnotify dir change), fileChangedMsg (fsnotify in-place
// Write), and the flush tick all funnel through probeDocCmd for the CURRENT
// view's document — the one case where nothing else already produced a fresh
// SyncState.

// probeResultMsg carries the outcome of an async store.Probe. docID/path are
// captured at issue time so the handler can reject a stale result (a later
// transition already changed the doc) — mirrors the H2 docID-recheck
// discipline used throughout the conflict-guard paths.
type probeResultMsg struct {
	docID   int64
	path    string
	state   docstate.SyncState
	missing bool
	err     error
}

// probeDocCmd issues an async store.Probe for docID, tagging the result with
// docID/path for the staleness check in handleProbeResult. A nil store or
// docID==0 (untitled/no doc yet) is a safe no-op (nil Cmd).
func probeDocCmd(store *docstate.Store, docID int64, path string) tea.Cmd {
	if store == nil || docID == 0 {
		return nil
	}
	return func() tea.Msg {
		state, err := store.Probe(docID)
		if err != nil {
			if errors.Is(err, fs.ErrNotExist) {
				return probeResultMsg{docID: docID, path: path, missing: true}
			}
			return probeResultMsg{docID: docID, path: path, err: err}
		}
		return probeResultMsg{docID: docID, path: path, state: state}
	}
}

// handleProbeResult applies a probeResultMsg: a missing target raises the
// GuardDeleted prompt (unless already up or the doc has no VFS record);
// otherwise the passive "changed on disk" hint tracks state.Kind ==
// DiskAhead||Diverged (F10) — NEVER Kind != Clean, which would also flag an
// ordinary unsaved edit (SyncBufferAhead: only the buffer changed, disk
// hasn't moved) as a false "changed on disk" warning. Dropped entirely — no
// guard, no hint mutation — when the result no longer applies to the live
// state (a later transition already changed the doc, closed it, or our own
// save is now in flight): a rendering/persistence tick must never nag the
// user about, or silently reconcile, the wrong document.
func (m Model) handleProbeResult(msg probeResultMsg) (Model, tea.Cmd) {
	if msg.docID != m.view.DocID() || msg.path != m.view.Path() || !m.view.IsFile() {
		return m, nil
	}
	if m.savingTarget(msg.docID, msg.path) {
		// Our own write is in flight for THIS document; a mismatch mid-write
		// is our own atomic rewrite in progress, not an external change
		// (§1.4.7). Targeted, not a blanket activeSave.InFlight check — an
		// unrelated tab's own save in flight must never suppress a probe
		// result for the document actually displayed now.
		return m, nil
	}
	if msg.err != nil {
		return m, nil // fire-and-forget: transient probe error, rung-3 only
	}
	if msg.missing {
		if !m.footer.InGuard() && !m.guard.deleted.active {
			m = m.raiseDeletedGuard(msg.docID, msg.path)
		}
		return m, nil
	}
	changed := msg.state.Kind == docstate.SyncDiskAhead || msg.state.Kind == docstate.SyncDiverged
	if changed == m.diskChangedHint {
		return m, nil
	}
	m = m.setDiskChangedHint(changed)
	text := ""
	if changed {
		text = "File changed on disk"
	}
	var cmd tea.Cmd
	m.footer, cmd = m.footer.Update(footer.ShowStatusMsg{Text: text})
	return m, cmd
}
