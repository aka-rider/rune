package workspace

import (
	"fmt"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/docstate"
	"rune/pkg/merge"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/pages/workspace/mergemode"
)

// ---- Guard raising -----------------------------------------------------------

// raiseConflictGuard stores guard.prompt and raises the GuardMerge footer
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
	m.guard.prompt = promptPayload{docID: docID, path: path, freshObs: theirsObs}
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
	m = m.raiseGuardPrompt(guardConflict)
	return m, cmd
}

// ---- Guard response handlers ------------------------------------------------
//
// W2: handleDataLossSaveAnyway/handleDataLossDiscardConflict/
// handleDataLossMerge are now pure consumers of the promptPayload the
// dispatcher (handleDataLossGuardResponse) already captured from guard.prompt
// and cleared — no more "if !active" defensive prologue (the dispatcher only
// calls these from the guardConflict case, so a mismatched guard.kind is
// unreachable here) and no more per-handler abandonDirtyContinuation call
// (hoisted once into the dispatcher's guardConflict case, since all three
// handlers called it identically — critic R1's coexistence-then-abandon
// semantics, unconditional here, unchanged in behavior).

// handleDataLossSaveAnyway handles the [S]ave-anyway response for the
// conflict guard: force-writes the LIVE editor buffer (not the snapshot
// captured at conflict-detection time — dictation/other async edits can
// reach the buffer between conflict detection and the [S] press; writing
// anything else would silently discard those keystrokes, rung 1) via
// store.Materialize with expect=p.freshObs — the CAS check accepts the
// write IFF disk still matches the conflicting bytes we already saw; if it
// changed AGAIN, Materialize raises a fresh conflict rather than blindly
// overwriting (strictly safer than the pre-v4 empty-baseline bypass, which
// skipped the CAS check entirely).
func (m Model) handleDataLossSaveAnyway(p promptPayload) (Model, tea.Cmd) {
	if m.store == nil {
		return m, nil
	}
	// Fix D: clear the guard-time preview — [S]ave anyway never enters the
	// resolver, so nothing should linger in the merge-view instance.
	m.merge = mergemode.Reset(m.merge)

	liveContent := m.editor.Content()
	var cmd tea.Cmd
	m, _, cmd = m.issueSave(saveReq{prefix: "force-save", docID: p.docID, path: p.path, content: liveContent, expect: p.freshObs, track: true})
	return m, cmd
}

// handleDataLossDiscardConflict handles the [D]iscard response for the
// conflict guard. Fix A (two-phase): rather than discarding onto bytes
// cached at detection time (possibly stale or now deleted — the merge
// data-race), it launches an async FRESH Probe; the actual buffer
// replacement happens in applyDiscardConflict once that read lands
// (workspace_merge_fresh.go). The VFS journal is NOT touched by this call;
// ours edits survive in the store and can be recovered manually.
func (m Model) handleDataLossDiscardConflict(p promptPayload) (Model, tea.Cmd) {
	if m.store == nil {
		return m, nil
	}
	// Ticket captured NOW, at the key press (Part IV) — p.docID (the
	// conflict's own target) paired with the CURRENT epoch, not
	// m.currentTicket()'s m.view.DocID(): the conflict guard always targets
	// what was displayed when it was raised, but pairing explicitly (rather
	// than re-deriving docID from m.view) keeps this correct even in the
	// vanishingly unlikely case they've since diverged.
	return m, resolveProbeCmd(m.store, viewTicket{docID: p.docID, epoch: m.epoch}, p.path, "", mergeIntentDiscard)
}

// handleDataLossMerge handles the [M]erge response for the conflict guard.
// Fix A (two-phase): captures the LIVE ours buffer synchronously (mirrors
// the [S]ave-anyway live-buffer fix) and launches an async FRESH Probe; the
// actual MergeHunks + mergemode.Enter happens in applyMergeConflict once
// that read lands (workspace_merge_fresh.go). A vanished target is caught
// there too, routing to the deleted guard instead of merging against bytes
// that no longer exist.
func (m Model) handleDataLossMerge(p promptPayload) (Model, tea.Cmd) {
	if m.store == nil {
		return m, nil
	}
	ours := m.editor.Content()
	// Ticket captured NOW, at the key press (Part IV) — see the [D]iscard
	// handler's comment above for why p.docID is paired explicitly rather
	// than re-derived via m.currentTicket().
	return m, resolveProbeCmd(m.store, viewTicket{docID: p.docID, epoch: m.epoch}, p.path, ours, mergeIntentMerge)
}

// ---- Guard response dispatch ------------------------------------------------

// handleDataLossGuardResponse dispatches a DataLossGuardResponseMsg to the
// appropriate sub-handler. Returns done=true when the response triggers an
// immediate application teardown/quit; in that case the cmds slice already
// contains the quit sequence and the caller must return without the broadcast
// section. Extracted from Update to keep workspace_update.go under the 500-LoC
// limit (§1.6/§11).
//
// A3 dispatches on guard.kind FIRST for conflict/deleted/raced/trash/
// degraded — the ×5 ".active"/kind prologue probes the pre-A3 shape needed
// (including the deleted-before-conflict priority guesswork the old
// DataLossDiscard/DataLossSaveAnyway cases hard-coded) all dissolve, since
// guard.kind alone now says which (if any) of these five guards is showing;
// an unrecognized (kind, response) combination is simply unreachable — every
// response footer can ever emit is drawn from THAT kind's own guardSpecs
// options table (workspace_guard.go), so there is no "illegal pair" to
// surface an error for (§1.3 is satisfied by construction, not a runtime
// check). A4 extends the SAME kind-first switch to
// guardDirtyClose/guardDirtyEvict/guardDirtyQuit (W1: the one continuation
// slot, guard.cont, discriminated by contKind) — the former free-standing
// "switch msg.Response{}" tail keyed on pendingDataLoss.kind, including its
// dangerous unguarded `default: // actionQuit` catch-all (reachable for ANY
// kind that fell through, not just a genuine quit), is gone: each of the
// three now has its own explicit case, so a DataLossDiscard can never be
// misrouted to teardownAndQuit just because guard.kind read something
// unexpected.
//
// W2 further collapses the trash/conflict/deleted/raced payload structs into
// guard.prompt (one slot, promptPayload) — each kind case that consumes it
// now does `p := m.guard.prompt; m = m.clearGuardPrompt()` exactly once
// (conflict/deleted also hoist the shared abandonDirtyContinuation call
// there, since all their sub-handlers used to call it identically), THEN
// dispatches to a pure `handleX(p)` consumer. guardDegraded is the one
// exception (deliberately, unchanged from A4): its [Y]es resumes the SAME
// close/evict/quit-save continuation the degraded-store check interrupted,
// so it must NOT abandon.
//
// Critic R1: a conflict guard raised DURING a live close/evict/quit save
// continuation is a legal, exercised state
// (TestConflictDuringCloseSave_CoexistsThenAbandonsClose) — guard.kind reads
// guardConflict in that window (raiseConflictGuard overwrote it), so this
// dispatcher correctly routes [S]/[D]/[M] to the conflict handlers, never
// back into the close/evict/quit cases below; those handlers rely on the
// hoisted abandonDirtyContinuation call in the guardConflict case to abandon
// the close/evict/quit continuation exactly as they did before this rewrite.
func (m Model) handleDataLossGuardResponse(msg footer.DataLossGuardResponseMsg, cmds []tea.Cmd) (Model, []tea.Cmd, bool) {
	var cmd tea.Cmd

	// Cancel is shared, uniform, and kind-agnostic — its blanket-clear body
	// never varies by which guard was showing (dirty/merge/deleted/trash/
	// raced/degraded), so it is checked FIRST, before the kind-first switch
	// below ever needs to reason about it. W2: clearGuardPrompt's single
	// guard.prompt wholesale-clear now covers what used to be four separate
	// per-kind clears (trashPath/conflict/deleted/raced) here.
	if msg.Response == footer.DataLossCancel {
		// Wholesale-clear the close/evict/quit continuation slot — mirrors
		// the pre-A4 `pendingDataLoss = pendingDataLoss{}` single-field reset
		// exactly (see abandonDirtyContinuation's own doc comment).
		m = m.abandonDirtyContinuation()
		// Clear the guard-time preview along with guard.prompt (Fix D) — Esc
		// never enters the resolver, so nothing should linger in the
		// merge-view. GUARDED on !IsActive: this DataLossCancel case is
		// shared by EVERY guard kind's Esc (dirty/merge/deleted/trash/raced),
		// not just GuardMerge — e.g. the user can switch to the file tree and
		// Cancel an UNRELATED GuardTrash prompt while a real merge is active
		// elsewhere (neither FocusExplorer nor FileDeleteRequestedMsg is
		// gated on HasUnresolvedConflicts). Preview never sets active=true,
		// so Reset is safe whenever mergemode is NOT actively resolving;
		// skipping it while active protects a genuine resolver session from
		// being wiped by an unrelated guard's Cancel. (A raced guard's Esc
		// keeps our already-committed write — F5 — nothing is undone by
		// dismissing; the displaced bytes stay reachable as history
		// regardless, equivalent to keep-mine.)
		if !mergemode.IsActive(m.merge) {
			m.merge = mergemode.Reset(m.merge)
		}
		// Explicitly clear the guard: in production the footer already cleared
		// it before emitting this message; in tests that inject the response
		// directly the footer may not have, so clear it here unconditionally.
		m = m.clearGuardPrompt()
		return m, cmds, false
	}

	switch m.guard.kind {
	case guardTrash:
		if msg.Response == footer.DataLossTrash {
			path := m.guard.prompt.path
			m = m.clearGuardPrompt()
			m.filetree = m.filetree.RemoveEntry(path)
			cmds = append(cmds, fileTrashCmd(m.fsys(), path))
		}
		return m, cmds, false

	case guardDeleted:
		p := m.guard.prompt
		// Hoisted R1 abandon (see this function's doc comment) — was
		// duplicated identically inside handleDeletedSave/handleDeletedDiscard.
		m = m.abandonDirtyContinuation()
		m = m.clearGuardPrompt()
		switch msg.Response {
		case footer.DataLossSaveAnyway:
			// GuardDeleted [S]ave = recreate the missing file from the live buffer.
			m, cmd = m.handleDeletedSave(p)
			cmds = append(cmds, cmd)
		case footer.DataLossDiscard:
			// GuardDeleted [D]iscard = purge doc history + close tab (§1.4.4
			// explicit choice).
			m, cmd = m.handleDeletedDiscard(p)
			cmds = append(cmds, cmd)
		}
		return m, cmds, false

	case guardConflict:
		p := m.guard.prompt
		// Hoisted R1 abandon (see this function's doc comment) — was
		// duplicated identically inside handleDataLossSaveAnyway/
		// handleDataLossDiscardConflict/handleDataLossMerge.
		m = m.abandonDirtyContinuation()
		m = m.clearGuardPrompt()
		switch msg.Response {
		case footer.DataLossSaveAnyway:
			// [S]ave anyway: clobber the external version with our buffer via a
			// CAS write against the captured conflict observation.
			m, cmd = m.handleDataLossSaveAnyway(p)
			cmds = append(cmds, cmd)
		case footer.DataLossDiscard:
			// DataLossDiscard for the conflict guard ([D]iscard = load theirs).
			m, cmd = m.handleDataLossDiscardConflict(p)
			cmds = append(cmds, cmd)
		case footer.DataLossMerge:
			// [M]erge: run 3-way merge (libgit2), enter the resolver UI.
			m, cmd = m.handleDataLossMerge(p)
			cmds = append(cmds, cmd)
		}
		return m, cmds, false

	case guardRaced:
		p := m.guard.prompt
		m = m.clearGuardPrompt()
		switch msg.Response {
		case footer.DataLossKeepMine:
			// F5: our write already committed for real — nothing left to do
			// at the store level, the guard is already cleared above.
		case footer.DataLossRestoreTheirs:
			// F5: write the captured displaced bytes back to disk, on top of
			// our already-committed write.
			m, cmd = m.handleDataLossRestoreTheirs(p)
			cmds = append(cmds, cmd)
		}
		return m, cmds, false

	case guardDegraded:
		if msg.Response == footer.DataLossConfirmDegraded {
			m = m.clearGuardPrompt()
			// Degraded-store guard (WP-R4 item 5): re-enter startSave bypassing
			// ONLY this one check — every other gate (unresolved conflicts,
			// pending load, divergence) still re-evaluates fresh. Deliberately
			// does NOT call abandonDirtyContinuation — its [Y]es RESUMES the
			// close/evict/quit-save continuation this guard interrupted.
			m, cmd = m.startSaveDegradedConfirmed(true)
			cmds = append(cmds, cmd)
		}
		return m, cmds, false

	case guardDirtyClose:
		switch msg.Response {
		case footer.DataLossSave:
			// Untitled has no path to save to. Its work is durable in the VFS,
			// so keep the buffer and abort the close rather than lose anything.
			if !m.view.IsFile() {
				m.guard.cont = continuation{}
				m = m.clearGuardPrompt()
				m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: "Untitled — name it to save (its text is safe in history)"})
				cmds = append(cmds, cmd)
				return m, cmds, false
			}
			// Prompt clears (guardAwaitingSave), guard.cont stays active —
			// startSave stamps its requestID and cont.owns(contClose, ...)
			// correlation checks it to decide close (§5.5).
			m = m.confirmGuardSave()
			m, cmd = m.startSave()
			cmds = append(cmds, cmd)
		case footer.DataLossDiscard:
			m.guard.cont = continuation{}
			m = m.clearGuardPrompt()
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
		}
		return m, cmds, false

	case guardDirtyEvict:
		switch msg.Response {
		case footer.DataLossSave:
			// Evict: save the dirty background victim; FileSavedMsg closes it
			// + opens pending. Prompt clears, guard.cont stays active.
			m = m.confirmGuardSave()
			m, cmd = m.evictSave()
			cmds = append(cmds, cmd)
		case footer.DataLossDiscard:
			// Discard: close the victim (history stays in VFS; recoverable on reopen).
			action := m.guard.cont
			m.guard.cont = continuation{}
			m = m.clearGuardPrompt()
			var discardCmd tea.Cmd
			m, discardCmd = m.evictDiscard(action)
			cmds = append(cmds, discardCmd)
		}
		return m, cmds, false

	case guardDirtyQuit:
		switch msg.Response {
		case footer.DataLossSave:
			// Quit: materialize every dirty bound tab, then tear down. Prompt
			// clears, guard.cont stays active for the saveLeft countdown.
			m = m.confirmGuardSave()
			m, cmd = m.saveAllDirtyForQuit()
			cmds = append(cmds, cmd)
		case footer.DataLossDiscard:
			// Discard all — journaled work survives in the VFS.
			m.guard.cont = continuation{}
			m = m.clearGuardPrompt()
			quitM, quitCmd := m.teardownAndQuit()
			return quitM, append(cmds, quitCmd), true
		}
		return m, cmds, false
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
