package workspace

import (
	"fmt"

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
		// raiseDirtyGuard wholesale-replaces the continuation slot (mirrors
		// the pre-A4 `pendingDataLoss = pendingDataLoss{kind: actionEvict,
		// ...}` single-field overwrite exactly — see its own doc comment)
		// before raising THIS guard.
		m = m.raiseDirtyGuard(guardDirtyEvict, continuation{
			kind:            contEvict,
			victim:          victim,
			pendingOpenPath: path,
		})
		m.footer = m.footer.SetGuardLabel(fmt.Sprintf("Close %q — unsaved.", m.opentabs.NameOf(victim)))
		return m, nil, false
	}
	m.opentabs = m.opentabs.Close(victim)
	return m, nil, true
}

// evictSave initiates saving the dirty eviction victim. Called when the user
// chooses [S]ave in the eviction guard prompt. Every vet failure below exits
// through failContinuation (W3) — same abandon-guard-and-surface shape,
// texts verbatim.
func (m Model) evictSave() (Model, tea.Cmd) {
	if m.store == nil {
		return m.failContinuation("cannot save: VFS not available")
	}
	victim := m.guard.cont.victim
	// §1.4.8 re-derive: mirrors startSave/saveAllDirtyForQuit's own check — a
	// dismissed (Esc'd) GuardMerge on the victim leaves no cached flag, and
	// CAS's own expect would otherwise let this silently clobber theirs.
	v := m.vetSave(victim.DocID)
	if v.SyncErr != nil {
		// §1.3/WP-R4: refuse on a failed divergence check — never fall
		// through to a write the re-check could not vet (review finding).
		return m.failContinuation("cannot save: divergence check failed: " + v.SyncErr.Error())
	}
	if v.Sync.Kind == docstate.SyncDiverged {
		return m.failContinuation("cannot save: unresolved external change — open the tab to resolve it")
	}
	content, err := m.store.Content(victim.DocID)
	if err != nil {
		return m.failContinuation("cannot save: " + err.Error())
	}
	// Overwrite-intent (bindNew=false): a missing CAS baseline (§1.7 — no
	// ObsID(0) sentinel) is refused rather than passed into Materialize as a
	// meaningless expect.
	if !v.HasExpect {
		return m.failContinuation("cannot save: no prior disk baseline for this document")
	}
	var requestID string
	var cmd tea.Cmd
	m, requestID, cmd = m.issueSave(saveReq{prefix: "evict", docID: victim.DocID, path: victim.Path, content: content, expect: v.Expect})
	m.guard.cont.requestID = requestID
	return m, cmd
}

// evictDiscard closes the eviction victim without saving and opens the pending
// file. The victim's edit history stays in the VFS and is recoverable on reopen
// (no DeleteDoc — matching the existing ^w Discard behavior).
func (m Model) evictDiscard(action continuation) (Model, tea.Cmd) {
	m.opentabs = m.opentabs.Close(action.victim)
	return m.requestOpenPath(0, action.pendingOpenPath)
}

// evictSaveAck handles a successful eviction save: marks the victim clean,
// re-binds its VFS identity (the atomic swap changed the inode), closes the
// tab, and opens the pending file. Materialize's own commit tx already
// re-bound the inode/kind — Bind here is a no-op re-confirmation kept for
// symmetry with the pre-v4 shape; the store side of "clean" (saved_obs) was
// already advanced inside Materialize's commit.
func (m Model) evictSaveAck() (Model, tea.Cmd) {
	victim := m.guard.cont.victim
	pendingPath := m.guard.cont.pendingOpenPath
	m.opentabs = m.opentabs.SetDirty(victim, false)
	m.guard.cont = continuation{}
	m = m.clearGuardPrompt()
	m.opentabs = m.opentabs.Close(victim)
	return m.requestOpenPath(0, pendingPath)
}
