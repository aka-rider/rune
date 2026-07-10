package workspace

import (
	"fmt"
	"path/filepath"
	"time"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/docstate"
	"rune/pkg/ui/components/footer"
)

// teardownAndQuit runs the shared quit sequence: clear pending state, disable
// dictation, close the store, delete pasted images, and quit.
func (m Model) teardownAndQuit() (Model, tea.Cmd) {
	m.pendingDataLoss = pendingDataLoss{}
	m.dict = m.dict.Disable()
	if m.cancelWatch != nil {
		m.cancelWatch() // release the live directory watch before quitting
	}
	if m.store != nil {
		_ = m.store.Close() // fire-and-forget: best-effort flush before quit
	}
	return m, tea.Sequence(m.editor.DeleteAllImagesCmd(), tea.Quit)
}

// saveAllDirtyForQuit materializes every dirty BOUND tab to disk before quit:
// the current tab from the editor buffer, others from their VFS reconstruction.
// Untitled dirty tabs are left untouched — durable in the VFS and recoverable
// next launch (Fix 7 §6) — so quit never blocks on a never-named doc
// (Decision 2). Teardown happens once every materialize has acked. Each
// Materialize call carries its OWN CAS expectation (savedObsFor) and
// save-start-captured seq (currentSeqFor) — no separate baseline/backstop
// plumbing needed (WP5).
func (m Model) saveAllDirtyForQuit() (Model, tea.Cmd) {
	if m.store == nil {
		return m.teardownAndQuit()
	}
	var batch []tea.Cmd
	for i, h := range m.dirtyHandles() { // ground-truth (H3, §1.4.8) — never the cached opentabs flag alone
		if h.Path == "" {
			continue // untitled — nothing to write
		}
		isCurrent := h.Equal(m.view.Handle())
		if isCurrent && m.HasUnresolvedConflicts() {
			// Defense-in-depth (§4/§6): the primary refusal (ConfirmQuitMsg)
			// already blocks reaching here while unresolved — this only guards
			// against a marker buffer ever reaching Materialize if that
			// refusal is ever bypassed.
			continue
		}
		// §1.4.8 re-derive: a dismissed (Esc'd) GuardMerge leaves no cached
		// flag, so re-check fresh here too (mirrors startSave's own check) —
		// otherwise CAS's own expect (Load already advanced saved_obs to the
		// very sighting that revealed the divergence) would let this quit
		// silently clobber theirs with a stale/never-reconciled ours, exactly
		// the clobber the guard exists to prevent (§0/§1.4.7).
		v := m.vetSave(h.DocID)
		if v.SyncErr != nil {
			// §1.3/WP-R4: a failed divergence check aborts the quit exactly
			// like a detected divergence — quitting on an unvetted write is
			// the same silent-clobber risk (review finding). The doc stays
			// dirty and durable in the VFS.
			m.pendingDataLoss = pendingDataLoss{}
			var cmd tea.Cmd
			m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: fmt.Sprintf("quit aborted — divergence check failed for %q: %v", filepath.Base(h.Path), v.SyncErr)})
			return m, cmd
		}
		if v.Sync.Kind == docstate.SyncDiverged {
			// F7: abort the WHOLE quit at the first diverged doc found — a
			// v4-era regression silently `continue`d past it here and quit
			// anyway with the REST of the batch, leaving the user believing
			// "Save all" saved everything when it silently skipped one.
			// Restores v3's refuse-and-surface.
			return m.abortQuitForDivergence(h.DocID, h.Path, h.Equal(m.view.Handle()), v.Sync)
		}
		// Overwrite-intent (bindNew=false): a missing CAS baseline (§1.7 — no
		// ObsID(0) sentinel) skips this doc (left dirty, durable in the VFS)
		// rather than passing a meaningless expect into Materialize — mirrors
		// the "content reconstruction fails" skip just below.
		if !v.HasExpect {
			// §1.3/§1.4.4: never SILENTLY drop a doc from an explicit "Save
			// all" — the user walks away believing everything was written.
			// The work is durable in the VFS, but the quit must abort and
			// say so (same refuse-and-surface as the Sync gates above).
			m.pendingDataLoss = pendingDataLoss{}
			var cmd tea.Cmd
			m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: fmt.Sprintf("quit aborted — %q has no disk baseline to save against; open it to save explicitly", filepath.Base(h.Path))})
			return m, cmd
		}
		expect := v.Expect
		requestID := fmt.Sprintf("quitsave-%d-%d-%v", h.DocID, i, time.Now().UnixNano())
		seq := m.currentSeqFor(h.DocID)
		if isCurrent {
			// Current doc: use the live editor buffer.
			batch = append(batch, materializeStoreCmd(m.store, h.DocID, h.Path, m.editor.Content(), expect, seq, requestID, false))
			continue
		}
		// Non-current tab: reconstruct its bytes from the VFS. Never write
		// empty/stale over a real file if reconstruction fails — and never
		// silently drop the doc from an explicit "Save all" either
		// (§1.3/§1.4.4): abort the quit and surface it. The work stays safe
		// in the VFS.
		content, err := m.store.Content(h.DocID)
		if err != nil {
			m.pendingDataLoss = pendingDataLoss{}
			var cmd tea.Cmd
			m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: fmt.Sprintf("quit aborted — cannot reconstruct %q: %v", filepath.Base(h.Path), err)})
			return m, cmd
		}
		batch = append(batch, materializeStoreCmd(m.store, h.DocID, h.Path, content, expect, seq, requestID, false))
	}
	if len(batch) == 0 {
		return m.teardownAndQuit() // only untitled docs are dirty — quit now
	}
	m.pendingDataLoss = pendingDataLoss{kind: actionQuit, saveLeft: len(batch)}
	return m, tea.Batch(batch...)
}

// abortQuitForDivergence aborts a "Save all" quit at the first diverged doc
// it finds (F7): cancels the quit teardown (pendingDataLoss already cleared
// by the caller not having set it), focuses that doc, and raises its
// conflict guard so the user can resolve it before quitting again — v3's
// refuse-and-surface. isCurrent means the doc is ALREADY displayed (the
// guard raises directly from the live buffer, synchronously); otherwise a
// navigation is issued and handleFileLoadedMsg's own load-time conflict
// detection raises the guard once that reload completes — never trusting
// the STALE `sync` snapshot for the guard's own content on a doc that isn't
// even displayed yet (a background tab's disk state could have moved again
// by the time the reload lands).
func (m Model) abortQuitForDivergence(docID int64, path string, isCurrent bool, sync docstate.SyncState) (Model, tea.Cmd) {
	var noticeCmd tea.Cmd
	m.footer, noticeCmd = m.footer.Update(footer.ShowStatusMsg{
		Text: fmt.Sprintf("Quit aborted — %q changed on disk and needs resolving first", filepath.Base(path)),
	})
	if isCurrent {
		var guardCmd tea.Cmd
		m, guardCmd = m.raiseConflictGuard(docID, path, m.editor.Content(), sync.Theirs.Hash, sync.Theirs.Obs)
		return m, tea.Batch(noticeCmd, guardCmd)
	}
	var openCmd tea.Cmd
	m, openCmd = m.requestOpenPath(docID, path)
	return m, tea.Batch(noticeCmd, openCmd)
}
