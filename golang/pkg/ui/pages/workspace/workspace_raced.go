package workspace

import (
	"fmt"
	"path/filepath"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/docstate"
	"rune/pkg/ui/components/footer"
)

// raiseRacedGuard stores guard.prompt and raises the GuardRaced footer
// prompt. Pure bookkeeping — both observations (and their blobs) are ALREADY
// captured and committed by the time Materialize returns Raced:true (I1), so
// there is no async read needed to raise this guard, mirroring
// raiseConflictGuard's synchronicity.
func (m Model) raiseRacedGuard(docID int64, path string, saved, fresh docstate.Observation) Model {
	m.guard.prompt = promptPayload{docID: docID, path: path, saved: saved, fresh: fresh}
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
		m.racedQueue = make(map[int64]promptPayload, 1)
	}
	m.racedQueue[docID] = promptPayload{docID: docID, path: path, saved: saved, fresh: fresh}
	var cmd tea.Cmd
	m.footer, cmd = m.footer.Update(footer.ShowStatusMsg{
		Text: fmt.Sprintf("⚠ save of %q raced with a concurrent write — open it to resolve", filepath.Base(path)),
	})
	*cmds = append(*cmds, cmd)
	return m
}

// surfaceRaced is the shared raise-vs-queue decision every Raced outcome
// (W6) must make: raise the guard directly when docID is the CURRENTLY
// displayed document, otherwise queue it (raiseRacedGuard's restore-theirs
// journals against the displayed buffer only, so an undisplayed doc's guard
// must wait — drainRacedQueue raises it at load-settle). displayed is passed
// in rather than re-derived here, since each call site already computed the
// exact comparison its own context needs (stillDisplayed, msg.DocID ==
// m.view.DocID(), ...) — collapsing the "if displayed { raise } else {
// queue }" pair itself, not the comparison.
func (m Model) surfaceRaced(displayed bool, docID int64, path string, saved, fresh docstate.Observation, cmds *[]tea.Cmd) Model {
	if displayed {
		return m.raiseRacedGuard(docID, path, saved, fresh)
	}
	return m.queueRacedGuard(docID, path, saved, fresh, cmds)
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

// handleDataLossRestoreTheirs handles the [R]estore-theirs response for the
// Raced guard: writes the captured displaced bytes back to disk, on top of
// our already-committed write. Only meaningful (and only safe) while the
// raced document is still the one displayed — installs the displaced
// content as a REAL journaled transition (mirrors installDiskAhead/
// applyDiscardConflict's ours->theirs pattern, W4: through the shared
// installJournaled chokepoint) so store.Content tracks the buffer, then
// issues a normal Materialize save whose expect is naturally the CURRENT
// SavedObs (p.saved, since nothing else has moved it) — no special CAS
// handling needed, exactly as the plan specifies.
//
// W2: pure consumer of the promptPayload the dispatcher
// (handleDataLossGuardResponse) already captured from guard.prompt and
// cleared — no more "if !active" prologue. W4: the install closure below
// folds the blob-read (F4/§1.3: the displaced observation's blob is ALWAYS
// expected to be readable — I1 captured it durably at swap-race time, so a
// read failure means real corruption, never a legitimate absence) and the
// ReplaceAll into ONE unit installJournaled treats as its install step — its
// single ok=false signal unifies what used to be 3 separate re-arm sites
// (blob-read fail, ReplaceAll fail, journal fail) onto the one re-arm below.
// Accepted delta: the error text on ok=false is now the single-sourced
// "restore theirs: <err>" / "restore theirs refused for doc %d: ..." shape
// installJournaled emits for every caller, replacing this function's former
// path-specific "cannot restore theirs for %q: %v" wording.
func (m Model) handleDataLossRestoreTheirs(p promptPayload) (Model, tea.Cmd) {
	if m.store == nil {
		return m, nil
	}

	if p.docID == 0 || p.docID != m.view.DocID() {
		// The raced document is no longer displayed — restoring theirs would
		// mean journaling an edit against whatever IS displayed now, wrongly
		// attributing it. Refuse safely and MOVE the guard to the queue: it
		// re-raises when the doc is next displayed. §1.3: surfaced, never
		// silent, never lost.
		var cmds []tea.Cmd
		m = m.queueRacedGuard(p.docID, p.path, p.saved, p.fresh, &cmds)
		var cmd tea.Cmd
		m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{
			Text: fmt.Sprintf("cannot restore theirs for %q — no longer displayed; reopen it to resolve", p.path),
		})
		cmds = append(cmds, cmd)
		return m, tea.Batch(cmds...)
	}

	var cmds []tea.Cmd
	install := func(m Model) (Model, tea.Cmd, error) {
		displaced, err := m.blobFor(docstate.Version{Hash: p.fresh.BlobHash, Obs: p.fresh.ID, Valid: true})
		if err != nil {
			return m, nil, err
		}
		var cmd tea.Cmd
		m.editor, cmd, err = m.editor.ReplaceAll(displaced)
		return m, cmd, err
	}
	// bump=true: mirrors the pre-W4 unconditional bumpEpoch after a
	// successful ReplaceAll. sync passed as the zero value (docstate.SyncState{})
	// so resolveAdoptAt inside installJournaled is a deliberate no-op —
	// restore-theirs adopts via its own Materialize/issueSave below, not
	// ResolveAdopt.
	var ok bool
	m, ok = m.installJournaled(p.docID, "restore theirs", docstate.SyncState{}, true, install, &cmds)
	if !ok {
		// Re-arm: the keypress already consumed the footer guard (dispatcher,
		// W2) — every refusal above (blob-read, buffer install, journal
		// write), unified into installJournaled's single ok signal, re-arms it
		// so [R] is never a one-shot lost to a transient failure (review
		// finding, preserved).
		m = m.raiseRacedGuard(p.docID, p.path, p.saved, p.fresh)
		return m, tea.Batch(cmds...)
	}

	content := m.editor.Content()
	var saveCmd tea.Cmd
	m, _, saveCmd = m.issueSave(saveReq{prefix: "restore-theirs", docID: p.docID, path: p.path, content: content, expect: p.saved.ID, track: true})
	cmds = append(cmds, saveCmd)
	return m, tea.Batch(cmds...)
}
