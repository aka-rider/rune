package workspace

import (
	"fmt"
	"time"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/docstate"
	"rune/pkg/ui/components/footer"
)

// enforceTabLimit checks whether opening path (with docID) would exceed the
// tab limit. If the file is already open, or the tab count is below the limit,
// returns (m, nil, true) — caller proceeds normally.
//
// If the limit would be exceeded, it tries to evict a victim:
//   - No eligible victim → refuses the open; returns (m, errorCmd, false).
//   - Dirty victim → raises the guard prompt; returns (m, nil, false).
//   - Clean victim → evicts silently; returns (m, nil, true) — caller proceeds.
func (m Model) enforceTabLimit(docID int64, path string) (Model, tea.Cmd, bool) {
	if m.opentabs.HasTab(docID, path) || m.opentabs.Len() < tabLimit {
		return m, nil, true
	}
	victim, cachedDirty, found := m.opentabs.EvictionCandidate()
	if !found {
		var cmd tea.Cmd
		m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: "Tab limit reached — close or unpin a tab"})
		return m, cmd, false
	}
	// H3/§1.4.8: re-verify ground-truth before deciding silent-evict vs
	// prompt — EvictionCandidate's OWN selection prefers a cached-clean tab,
	// but that cache can drift; a candidate it picked as "clean" must never
	// be silently discarded if the store actually holds unsaved edits for
	// it (worst case here is an unnecessary prompt, never a lost edit).
	dirty := m.isDirtyGroundTruth(victim.DocID, cachedDirty)
	if dirty {
		m.pendingDataLoss = pendingDataLoss{
			kind:            actionEvict,
			victim:          victim,
			pendingOpenPath: path,
		}
		m.footer = m.footer.
			SetGuard(footer.GuardDirty, dataLossGuardOptions).
			SetGuardLabel(fmt.Sprintf("Close %q — unsaved.", m.opentabs.NameByID(victim.DocID)))
		return m, nil, false
	}
	m.opentabs = m.opentabs.CloseByID(victim.DocID)
	return m, nil, true
}

// evictSave initiates saving the dirty eviction victim. Called when the user
// chooses [S]ave in the eviction guard prompt.
func (m Model) evictSave() (Model, tea.Cmd) {
	if m.store == nil {
		m.pendingDataLoss = pendingDataLoss{}
		var cmd tea.Cmd
		m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: "cannot save: VFS not available"})
		return m, cmd
	}
	victim := m.pendingDataLoss.victim
	// §1.4.8 re-derive: mirrors startSave/saveAllDirtyForQuit's own check — a
	// dismissed (Esc'd) GuardMerge on the victim leaves no cached flag, and
	// CAS's own expect would otherwise let this silently clobber theirs.
	v := m.vetSave(victim.DocID)
	if v.SyncErr != nil {
		// §1.3/WP-R4: refuse on a failed divergence check — never fall
		// through to a write the re-check could not vet (review finding).
		m.pendingDataLoss = pendingDataLoss{}
		var cmd tea.Cmd
		m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: "cannot save: divergence check failed: " + v.SyncErr.Error()})
		return m, cmd
	}
	if v.Sync.Kind == docstate.SyncDiverged {
		m.pendingDataLoss = pendingDataLoss{}
		var cmd tea.Cmd
		m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: "cannot save: unresolved external change — open the tab to resolve it"})
		return m, cmd
	}
	content, err := m.store.Content(victim.DocID)
	if err != nil {
		m.pendingDataLoss = pendingDataLoss{}
		var cmd tea.Cmd
		m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: "cannot save: " + err.Error()})
		return m, cmd
	}
	// Overwrite-intent (bindNew=false): a missing CAS baseline (§1.7 — no
	// ObsID(0) sentinel) is refused rather than passed into Materialize as a
	// meaningless expect.
	if !v.HasExpect {
		m.pendingDataLoss = pendingDataLoss{}
		var cmd tea.Cmd
		m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: "cannot save: no prior disk baseline for this document"})
		return m, cmd
	}
	expect := v.Expect
	seq := m.currentSeqFor(victim.DocID)
	requestID := fmt.Sprintf("evict-%d-%v", victim.DocID, time.Now().UnixNano())
	m.pendingDataLoss.requestID = requestID
	return m, materializeStoreCmd(m.store, victim.DocID, victim.Path, content, expect, seq, requestID, false)
}

// evictDiscard closes the eviction victim without saving and opens the pending
// file. The victim's edit history stays in the VFS and is recoverable on reopen
// (no DeleteDoc — matching the existing ^w Discard behavior).
func (m Model) evictDiscard(action pendingDataLoss) (Model, tea.Cmd) {
	m.opentabs = m.opentabs.CloseByID(action.victim.DocID)
	return m.requestOpenPath(0, action.pendingOpenPath)
}

// isEvictSaveAck reports whether a requestID is the ack for the pending
// eviction background save.
func (m Model) isEvictSaveAck(requestID string) bool {
	return m.pendingDataLoss.kind == actionEvict &&
		m.pendingDataLoss.requestID != "" &&
		requestID == m.pendingDataLoss.requestID
}

// isCloseSaveAck reports whether a requestID is the ack for the save startSave
// launched as the confirmed continuation of a pending actionClose guard
// (footer.DataLossSave -> startSave, which stamps pendingDataLoss.requestID —
// workspace_edit.go). An UNSTAMPED actionClose (requestID=="") is a guard that
// some OTHER, unrelated save's ack must never clear — GUARD-STATE-COH: a save
// with no idea a pendingDataLoss action exists (bind-new, restore-theirs) can
// never accidentally "own" and clear someone else's still-pending guard.
func (m Model) isCloseSaveAck(requestID string) bool {
	return m.pendingDataLoss.kind == actionClose &&
		m.pendingDataLoss.requestID != "" &&
		requestID == m.pendingDataLoss.requestID
}

// evictSaveAck handles a successful eviction save: marks the victim clean,
// re-binds its VFS identity (the atomic swap changed the inode), closes the
// tab, and opens the pending file. Materialize's own commit tx already
// re-bound the inode/kind — Bind here is a no-op re-confirmation kept for
// symmetry with the pre-v4 shape; the store side of "clean" (saved_obs) was
// already advanced inside Materialize's commit.
func (m Model) evictSaveAck() (Model, tea.Cmd) {
	victim := m.pendingDataLoss.victim
	pendingPath := m.pendingDataLoss.pendingOpenPath
	m.opentabs = m.opentabs.MarkCleanByID(victim.DocID)
	m.pendingDataLoss = pendingDataLoss{}
	m.opentabs = m.opentabs.CloseByID(victim.DocID)
	return m.requestOpenPath(0, pendingPath)
}
