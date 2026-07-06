package workspace

import (
	"fmt"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/docstate"
	"rune/pkg/merge"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/pages/workspace/mergemode"
)

// ---- Pending-conflict state -------------------------------------------------

// pendingConflict holds the data collected when a FileSaveErrorMsg{Conflict:true}
// (or a load-time / undo-unwind divergence) is detected for the current
// document. freshObs is the conflicting disk observation Materialize (or
// Probe) already captured via I1 (capture-before-discard) — [S]ave-anyway
// passes it straight back as the NEW CAS `expect`, and [D]/[M] re-probe fresh
// before acting (Fix A: the file may have moved again since detection).
// Zero value = no active conflict.
type pendingConflict struct {
	active   bool
	path     string
	docID    int64
	freshObs docstate.ObsID
}

// ---- Guard raising -----------------------------------------------------------

// raiseConflictGuard stores pendingConflict and raises the GuardMerge footer
// prompt. Pure SQLite (GetBlob + Sync for the ancestor) — no disk I/O, so this
// is always synchronous, unlike the pre-v4 theirsReadCmd round trip: the
// conflicting bytes are ALREADY captured (I1) by whatever produced theirsHash
// (Materialize's Fresh observation, or Load's/Probe's SyncState.Theirs).
// Called from the save-conflict path (FileSaveErrorMsg), the load-time
// conflict path (handleFileLoadedMsg), and the undo-unwind re-check
// (handleUnwindProbe) to keep every entry point DRY.
//
// A GetBlob failure here (F4 sweep, §1.3) is surfaced via the returned Cmd
// rather than silently substituted with "" — this only affects the
// PREVIEW (mergemode.Preview never writes anything), so the guard still
// raises (the user needs a path to resolve the conflict either way); the
// DESTRUCTIVE resolutions ([D]/[M]) always re-read fresh via
// resolveProbeCmd/blobFor, which refuses outright on the same failure.
func (m Model) raiseConflictGuard(docID int64, path, oursContent, theirsHash string, theirsObs docstate.ObsID) (Model, tea.Cmd) {
	var ancestorContent, theirsContent string
	var cmd tea.Cmd
	if m.store != nil {
		if theirsHash != "" {
			if c, err := m.store.GetBlob(theirsHash); err == nil {
				theirsContent = c
			} else {
				m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: fmt.Sprintf("theirs blob unreadable: %v", err)})
			}
		}
		if sync, err := m.store.Sync(docID); err == nil && sync.Ancestor.Valid {
			if c, err := m.store.GetBlob(sync.Ancestor.Hash); err == nil {
				ancestorContent = c
			} else if cmd == nil {
				m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: fmt.Sprintf("ancestor blob unreadable: %v", err)})
			}
		}
	}
	m.pendingConflict = pendingConflict{active: true, path: path, docID: docID, freshObs: theirsObs}
	// Fix D (BUG2): build the read-only ours-vs-theirs preview NOW, from the
	// SAME 3-way merge [M] will (re)run, so the guard is reviewable BEFORE the
	// user picks [S]/[D]/[M] — including for a clean (zero-conflict) auto-merge,
	// which is otherwise invisible (the "no [O]/[T] GUI" case — the preview IS
	// the review step there, per the plan's resolution). On a libgit2 failure,
	// degrade to an empty preview rather than block the guard from being
	// raised at all — [M] will surface the real error if it recurs.
	hunks, err := merge.MergeHunks([]byte(ancestorContent), []byte(oursContent), []byte(theirsContent))
	if err != nil {
		hunks = nil
	}
	m.merge = mergemode.Preview(hunks, m.merge)
	// Raise the GuardMerge prompt. Cancel is last (§1.4.4 / R4).
	m.footer = m.footer.SetGuard(footer.GuardMerge, guardMergeOptions)
	return m, cmd
}

// ---- Guard response handlers ------------------------------------------------

// handleDataLossSaveAnyway handles the [S]ave-anyway response for the
// conflict guard: force-writes the LIVE editor buffer (not the snapshot
// captured at conflict-detection time — dictation/other async edits can
// reach the buffer between conflict detection and the [S] press; writing
// anything else would silently discard those keystrokes, rung 1) via
// store.Materialize with expect=pc.freshObs — the CAS check accepts the
// write IFF disk still matches the conflicting bytes we already saw; if it
// changed AGAIN, Materialize raises a fresh conflict rather than blindly
// overwriting (strictly safer than the pre-v4 empty-baseline bypass, which
// skipped the CAS check entirely).
func (m Model) handleDataLossSaveAnyway() (Model, tea.Cmd) {
	if !m.pendingConflict.active || m.store == nil {
		m.pendingConflict = pendingConflict{}
		m.pendingDataLoss = pendingDataLoss{}
		return m, nil
	}
	pc := m.pendingConflict
	m.pendingConflict = pendingConflict{}
	m.pendingDataLoss = pendingDataLoss{}
	// Fix D: clear the guard-time preview — [S]ave anyway never enters the
	// resolver, so nothing should linger in the merge-view instance.
	m.merge = mergemode.Reset(m.merge)

	liveContent := m.editor.Content()
	seq := m.currentSeqFor(pc.docID)
	requestID := fmt.Sprintf("force-save-%d", pc.docID)
	m.activeSave = SaveIdentity{RequestID: requestID, SavedContent: []byte(liveContent), InFlight: true, Path: pc.path, DocID: pc.docID}
	return m, materializeStoreCmd(m.store, pc.docID, pc.path, liveContent, pc.freshObs, seq, requestID, false)
}

// handleDataLossDiscardConflict handles the [D]iscard response for the
// conflict guard. Fix A (two-phase): rather than discarding onto bytes
// cached at detection time (possibly stale or now deleted — the merge
// data-race), it clears pendingConflict and launches an async FRESH Probe;
// the actual buffer replacement happens in applyDiscardConflict once that
// read lands (workspace_merge_fresh.go). The VFS journal is NOT touched by
// this call; ours edits survive in the store and can be recovered manually.
func (m Model) handleDataLossDiscardConflict() (Model, tea.Cmd) {
	if !m.pendingConflict.active || m.store == nil {
		m.pendingConflict = pendingConflict{}
		m.pendingDataLoss = pendingDataLoss{}
		return m, nil
	}
	pc := m.pendingConflict
	m.pendingConflict = pendingConflict{}
	m.pendingDataLoss = pendingDataLoss{}
	// Ticket captured NOW, at the key press (Part IV) — pc.docID (the
	// conflict's own target) paired with the CURRENT epoch, not
	// m.currentTicket()'s m.view.DocID(): pendingConflict always targets what
	// was displayed when it was raised, but pairing explicitly (rather than
	// re-deriving docID from m.view) keeps this correct even in the
	// vanishingly unlikely case they've since diverged.
	return m, resolveProbeCmd(m.store, viewTicket{docID: pc.docID, epoch: m.epoch}, pc.path, "", mergeIntentDiscard)
}

// handleDataLossMerge handles the [M]erge response for the conflict guard.
// Fix A (two-phase): captures the LIVE ours buffer synchronously (mirrors
// the [S]ave-anyway live-buffer fix) and launches an async FRESH Probe; the
// actual MergeHunks + mergemode.Enter happens in applyMergeConflict once
// that read lands (workspace_merge_fresh.go). A vanished target is caught
// there too, routing to the deleted guard instead of merging against bytes
// that no longer exist.
func (m Model) handleDataLossMerge() (Model, tea.Cmd) {
	if !m.pendingConflict.active || m.store == nil {
		m.pendingConflict = pendingConflict{}
		m.pendingDataLoss = pendingDataLoss{}
		return m, nil
	}
	pc := m.pendingConflict
	m.pendingConflict = pendingConflict{}
	m.pendingDataLoss = pendingDataLoss{}
	ours := m.editor.Content()
	// Ticket captured NOW, at the key press (Part IV) — see the [D]iscard
	// handler's comment above for why pc.docID is paired explicitly rather
	// than re-derived via m.currentTicket().
	return m, resolveProbeCmd(m.store, viewTicket{docID: pc.docID, epoch: m.epoch}, pc.path, ours, mergeIntentMerge)
}

// ---- Guard response dispatch ------------------------------------------------

// handleDataLossGuardResponse dispatches a DataLossGuardResponseMsg to the
// appropriate sub-handler. Returns done=true when the response triggers an
// immediate application teardown/quit; in that case the cmds slice already
// contains the quit sequence and the caller must return without the broadcast
// section. Extracted from Update to keep workspace_update.go under the 500-LoC
// limit (§1.6/§11).
func (m Model) handleDataLossGuardResponse(msg footer.DataLossGuardResponseMsg, cmds []tea.Cmd) (Model, []tea.Cmd, bool) {
	var cmd tea.Cmd

	switch msg.Response {
	case footer.DataLossTrash:
		if m.pendingDataLoss.kind == actionTrash {
			path := m.pendingDataLoss.pendingTrashPath
			m.pendingDataLoss = pendingDataLoss{}
			m.filetree = m.filetree.RemoveEntry(path)
			cmds = append(cmds, fileTrashCmd(m.fsys(), path))
		}

	case footer.DataLossSave:
		if m.pendingDataLoss.kind == actionEvict {
			// Evict: save the dirty background victim; FileSavedMsg closes it + opens pending.
			m, cmd = m.evictSave()
			cmds = append(cmds, cmd)
			break
		}
		if m.pendingDataLoss.kind == actionQuit {
			// Quit: materialize every dirty bound tab, then tear down.
			m, cmd = m.saveAllDirtyForQuit()
			cmds = append(cmds, cmd)
			break
		}
		// Close (or stray): save the current tab; FileSavedMsg closes it.
		if !m.view.IsFile() {
			// Untitled has no path to save to. Its work is durable in the VFS,
			// so keep the buffer and abort the close rather than lose anything.
			m.pendingDataLoss = pendingDataLoss{}
			m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: "Untitled — name it to save (its text is safe in history)"})
			cmds = append(cmds, cmd)
			break
		}
		m, cmd = m.startSave()
		cmds = append(cmds, cmd)
		// pendingDataLoss preserved — FileSavedMsg checks it to decide close.

	case footer.DataLossDiscard:
		// GuardDeleted [D]iscard = purge doc history + close tab (§1.4.4 explicit
		// choice). Checked BEFORE pendingConflict so it never misroutes to the
		// conflict guard's discard (load theirs — there is no theirs here).
		if m.pendingDeleted.active {
			var deletedDiscardCmd tea.Cmd
			m, deletedDiscardCmd = m.handleDeletedDiscard()
			cmds = append(cmds, deletedDiscardCmd)
			break
		}
		// DataLossDiscard for the conflict guard ([D]iscard = load theirs).
		// Distinguish from the dirty-guard discard by checking pendingConflict.
		if m.pendingConflict.active {
			var discardCmd tea.Cmd
			m, discardCmd = m.handleDataLossDiscardConflict()
			cmds = append(cmds, discardCmd)
			break
		}
		action := m.pendingDataLoss
		m.pendingDataLoss = pendingDataLoss{}
		switch action.kind {
		case actionClose:
			// Discarding an untitled removes its VFS doc so it is not offered
			// for recovery later (Fix 7 §6); a bound doc keeps its history.
			if m.view.IsUntitled() && m.view.DocID() != 0 && m.store != nil {
				if err := m.store.DeleteDoc(m.view.DocID()); err != nil {
					_ = err // fire-and-forget: discard cleanup; non-fatal
				}
			}
			var closeCmd tea.Cmd
			m, closeCmd = m.executeClose(m.view.DocID(), m.view.Path())
			cmds = append(cmds, closeCmd)
		case actionEvict:
			// Discard: close the victim (history stays in VFS; recoverable on reopen).
			var discardCmd tea.Cmd
			m, discardCmd = m.evictDiscard(action)
			cmds = append(cmds, discardCmd)
		default: // actionQuit: discard all — journaled work survives in the VFS
			quitM, quitCmd := m.teardownAndQuit()
			return quitM, append(cmds, quitCmd), true
		}

	case footer.DataLossCancel:
		m.pendingDataLoss = pendingDataLoss{}
		// Clear pendingConflict on Esc (R2): a later dirty-guard [D]iscard must
		// route to discard-and-close, not to handleDataLossDiscardConflict (which
		// loads theirs). Save-gating after Esc lives in the Probe-driven SyncState
		// re-check on the next save attempt, not in pendingConflict.
		m.pendingConflict = pendingConflict{}
		// Fix D: clear the guard-time preview along with pendingConflict — Esc
		// never enters the resolver, so nothing should linger in the merge-view.
		// GUARDED on !IsActive: this DataLossCancel case is shared by EVERY guard
		// kind's Esc (dirty/merge/deleted/trash), not just GuardMerge — e.g. the
		// user can switch to the file tree and Cancel an UNRELATED GuardTrash
		// prompt while a real merge is active elsewhere (neither FocusExplorer
		// nor FileDeleteRequestedMsg is gated on HasUnresolvedConflicts). Preview
		// never sets active=true, so Reset is safe whenever mergemode is NOT
		// actively resolving; skipping it while active protects a genuine
		// resolver session from being wiped by an unrelated guard's Cancel.
		if !mergemode.IsActive(m.merge) {
			m.merge = mergemode.Reset(m.merge)
		}
		// Clear pendingDeleted on Esc: keep editing with the buffer intact; the
		// guard re-raises only on a fresh deletion signal (dirChangedMsg / stat
		// check), never on every save (the doc stays dirty and a later ⌘S
		// recreates the file via the normal overwrite path).
		m.pendingDeleted = pendingDeleted{}
		// Clear pendingRaced on Esc: our write already committed physically
		// (F5) — nothing is undone by dismissing; the displaced bytes stay
		// reachable as history regardless (equivalent to keep-mine).
		m = m.handleDataLossKeepMine()
		// Explicitly clear the guard: in production the footer already cleared
		// it before emitting this message; in tests that inject the response
		// directly the footer may not have, so clear it here unconditionally.
		m.footer = m.footer.SetGuard(footer.GuardDirty, nil)

	case footer.DataLossKeepMine:
		// F5: our write already committed for real — nothing to do at the
		// store level, just clear the guard.
		m = m.handleDataLossKeepMine()

	case footer.DataLossRestoreTheirs:
		// F5: write the captured displaced bytes back to disk, on top of our
		// already-committed write.
		var restoreCmd tea.Cmd
		m, restoreCmd = m.handleDataLossRestoreTheirs()
		cmds = append(cmds, restoreCmd)

	case footer.DataLossConfirmDegraded:
		// Degraded-store guard (WP-R4 item 5): re-enter startSave bypassing
		// ONLY this one check — every other gate (unresolved conflicts,
		// pending load, divergence) still re-evaluates fresh.
		var saveCmd tea.Cmd
		m, saveCmd = m.startSaveDegradedConfirmed(true)
		cmds = append(cmds, saveCmd)

	case footer.DataLossSaveAnyway:
		// GuardDeleted [S]ave = recreate the missing file from the live buffer.
		if m.pendingDeleted.active {
			var deletedSaveCmd tea.Cmd
			m, deletedSaveCmd = m.handleDeletedSave()
			cmds = append(cmds, deletedSaveCmd)
			break
		}
		// [S]ave anyway: clobber the external version with our buffer via a
		// CAS write against the captured conflict observation.
		var saveAnywayCmd tea.Cmd
		m, saveAnywayCmd = m.handleDataLossSaveAnyway()
		cmds = append(cmds, saveAnywayCmd)

	case footer.DataLossMerge:
		// [M]erge: run 3-way merge (libgit2), enter the resolver UI.
		var mergeCmd tea.Cmd
		m, mergeCmd = m.handleDataLossMerge()
		cmds = append(cmds, mergeCmd)
	}

	return m, cmds, false
}

// ---- R2 save-gating ---------------------------------------------------------

// HasUnresolvedConflicts returns true when the merge resolver is active and
// still has unresolved conflict blocks. startSave and the modal-transition
// guards (§4) check this to refuse writing/backgrounding/closing/quitting a
// mid-merge doc (R2).
func (m Model) HasUnresolvedConflicts() bool {
	return mergemode.IsActive(m.merge) && mergemode.HasUnresolvedConflicts(m.merge)
}
