package workspace

import (
	"fmt"
	"path/filepath"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/docstate"
	"rune/pkg/editor/buffer"
	"rune/pkg/ui/components/footer"
)

// racedIntent holds the two competing observations a Materialize swap-race
// (F5, MatResult{Committed:true, Raced:true}) produced: saved is OUR write,
// already physically on disk and already the CAS baseline (commitSave ran
// for it); fresh is the concurrent writer's displaced bytes, captured
// per I1 but not (yet) written anywhere. active is the out-of-band validity
// bit (§1.7). Lives at guardState.raced (A3) — see workspace.go's Model.guard
// doc comment for why this is a guard distinct from guard.conflict.
type racedIntent struct {
	active bool
	path   string
	docID  int64
	saved  docstate.Observation
	fresh  docstate.Observation
}

// raiseRacedGuard stores guard.raced and raises the GuardRaced footer
// prompt. Pure bookkeeping — both observations (and their blobs) are ALREADY
// captured and committed by the time Materialize returns Raced:true (I1), so
// there is no async read needed to raise this guard, mirroring
// raiseConflictGuard's synchronicity.
func (m Model) raiseRacedGuard(docID int64, path string, saved, fresh docstate.Observation) Model {
	m.guard.raced = racedIntent{active: true, path: path, docID: docID, saved: saved, fresh: fresh}
	m.err = nil
	m = m.raiseGuardPrompt(guardRaced)
	return m
}

// queueRacedGuard records a raced-save outcome for a document that is NOT
// currently displayed (evict/quit-batch saves, a tab switched away while the
// save was in flight) and says so in the footer. raiseRacedGuard's
// restore-theirs journals against the DISPLAYED buffer, so the guard must
// wait until the doc is next displayed — drainRacedQueue raises it at
// load-settle. The queue is the bridge that keeps a background race from
// resolving silently (review finding: the displaced bytes were otherwise
// reachable only by manual blob archaeology).
func (m Model) queueRacedGuard(docID int64, path string, saved, fresh docstate.Observation, cmds *[]tea.Cmd) Model {
	if docID == 0 {
		return m
	}
	if m.racedQueue == nil {
		m.racedQueue = make(map[int64]racedIntent, 1)
	}
	m.racedQueue[docID] = racedIntent{active: true, path: path, docID: docID, saved: saved, fresh: fresh}
	var cmd tea.Cmd
	m.footer, cmd = m.footer.Update(footer.ShowStatusMsg{
		Text: fmt.Sprintf("⚠ save of %q raced with a concurrent write — open it to resolve", filepath.Base(path)),
	})
	*cmds = append(*cmds, cmd)
	return m
}

// drainRacedQueue raises a queued raced guard for docID if one is waiting.
// Called at load-settle (handleFileLoadedMsg), the one point where the doc
// is guaranteed displayed and identity has settled.
func (m Model) drainRacedQueue(docID int64) Model {
	if docID == 0 || m.racedQueue == nil {
		return m
	}
	pr, ok := m.racedQueue[docID]
	if !ok {
		return m
	}
	delete(m.racedQueue, docID)
	return m.raiseRacedGuard(pr.docID, pr.path, pr.saved, pr.fresh)
}

// handleDataLossKeepMine handles the [K]eep-mine response for the Raced
// guard: our write already committed for real (Materialize's own commitSave
// ran the moment the race was detected), so there is nothing left to DO at
// the store level — saved_obs already points at it. This just clears the
// guard; the displaced bytes remain reachable as history (never deleted)
// for the user to recover by hand later if they change their mind.
func (m Model) handleDataLossKeepMine() Model {
	m.guard.raced = racedIntent{}
	return m
}

// handleDataLossRestoreTheirs handles the [R]estore-theirs response for the
// Raced guard: writes the captured displaced bytes back to disk, on top of
// our already-committed write. Only meaningful (and only safe) while the
// raced document is still the one displayed — installs the displaced
// content as a REAL journaled transition (mirrors installDiskAhead/
// applyDiscardConflict's ours->theirs pattern) so store.Content tracks the
// buffer, then issues a normal Materialize save whose expect is naturally
// the CURRENT SavedObs (pc.saved, since nothing else has moved it) — no
// special CAS handling needed, exactly as the plan specifies.
func (m Model) handleDataLossRestoreTheirs() (Model, tea.Cmd) {
	if !m.guard.raced.active || m.store == nil {
		m.guard.raced = racedIntent{}
		return m, nil
	}
	// Validate BEFORE consuming the guard (review finding): clearing
	// guard.raced on a refused precondition made [R] a one-shot that could
	// be permanently lost to a transient state (e.g. the raced doc's reload
	// still in flight) — after which no UI path to the displaced bytes
	// remained. On refusal the guard stays armed for a retry.
	pr := m.guard.raced

	if pr.docID == 0 || pr.docID != m.view.DocID() {
		// The raced document is no longer displayed — restoring theirs would
		// mean journaling an edit against whatever IS displayed now, wrongly
		// attributing it. Refuse safely and MOVE the guard to the queue: it
		// re-raises when the doc is next displayed. §1.3: surfaced, never
		// silent, never lost.
		m.guard.raced = racedIntent{}
		var cmds []tea.Cmd
		m = m.queueRacedGuard(pr.docID, pr.path, pr.saved, pr.fresh, &cmds)
		var cmd tea.Cmd
		m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{
			Text: fmt.Sprintf("cannot restore theirs for %q — no longer displayed; reopen it to resolve", pr.path),
		})
		cmds = append(cmds, cmd)
		return m, tea.Batch(cmds...)
	}

	// F4/§1.3: the displaced observation's blob is ALWAYS expected to be
	// readable (I1 captured it durably at swap-race time) — a read failure
	// means real corruption, never a legitimate absence. Refuse rather than
	// restoring substituted-empty content over the buffer; the guard stays
	// armed (guard.raced NOT cleared) so [R] can be retried or [K] chosen.
	displaced, err := m.blobFor(docstate.Version{Hash: pr.fresh.BlobHash, Obs: pr.fresh.ID, Valid: true})
	if err != nil {
		// Re-raise: the keypress consumed the footer guard, so re-arm it
		// alongside the error — the choice ([R] retry / [K]eep) survives.
		m = m.raiseRacedGuard(pr.docID, pr.path, pr.saved, pr.fresh)
		var cmd tea.Cmd
		m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{
			Text: fmt.Sprintf("cannot restore theirs for %q: %v", pr.path, err),
		})
		return m, cmd
	}
	m.guard.raced = racedIntent{}

	var cmds []tea.Cmd
	prevCursors := m.editor.Cursors()
	var cmd tea.Cmd
	m.editor, cmd, err = m.editor.ReplaceAll(displaced)
	if err != nil {
		// Re-raise: the displaced blob is still safe in the store (I1); the
		// buffer install just failed to apply, so re-arm the guard rather than
		// silently proceeding as if the restore had happened (§1.3).
		m = m.raiseRacedGuard(pr.docID, pr.path, pr.saved, pr.fresh)
		var errCmd tea.Cmd
		m.footer, errCmd = m.footer.Update(footer.ShowErrorMsg{
			Text: fmt.Sprintf("cannot restore theirs for %q: %v", pr.path, err),
		})
		return m, errCmd
	}
	cmds = append(cmds, cmd)
	m = m.bumpEpoch() // Part IV: a restore-theirs ReplaceAll invalidates every outstanding view ticket
	var editorEdits []buffer.AppliedEdit
	m.editor, editorEdits = m.editor.DrainEdits()
	var ok bool
	m, ok = m.journalEditOK(targetMain, editorEdits, prevCursors, m.editor.Cursors(), &cmds)
	if !ok {
		// Journal append failed → buffer rolled back to our committed bytes;
		// proceeding to Materialize would stamp a save at a position that
		// does not include the restore (Adoption Contract). Re-arm the guard
		// so the user can retry; the error is already surfaced.
		m = m.raiseRacedGuard(pr.docID, pr.path, pr.saved, pr.fresh)
		return m, tea.Batch(cmds...)
	}

	content := m.editor.Content()
	seq := m.currentSeqFor(pr.docID)
	requestID := fmt.Sprintf("restore-theirs-%d", pr.docID)
	m.activeSave = SaveIdentity{RequestID: requestID, SavedContent: []byte(content), InFlight: true, Path: pr.path, DocID: pr.docID}
	cmds = append(cmds, materializeStoreCmd(m.store, pr.docID, pr.path, content, pr.saved.ID, seq, requestID, false))
	return m, tea.Batch(cmds...)
}
