package workspace

import (
	"fmt"
	"time"

	tea "charm.land/bubbletea/v2"

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
	victim, dirty, found := m.opentabs.EvictionCandidate()
	if !found {
		var cmd tea.Cmd
		m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: "Tab limit reached — close or unpin a tab"})
		return m, cmd, false
	}
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
	content, err := m.store.Content(victim.DocID)
	if err != nil {
		m.pendingDataLoss = pendingDataLoss{}
		var cmd tea.Cmd
		m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: "cannot save: " + err.Error()})
		return m, cmd
	}
	requestID := fmt.Sprintf("evict-%d-%v", victim.DocID, time.Now().UnixNano())
	m.pendingDataLoss.requestID = requestID
	return m, materializeCmd(victim.DocID, victim.Path, content, requestID, false, diskBaseline{})
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

// evictSaveAck handles a successful eviction save: marks the victim clean,
// re-binds its VFS identity (atomic write changed the inode), closes the tab,
// and opens the pending file.
func (m Model) evictSaveAck() (Model, tea.Cmd) {
	victim := m.pendingDataLoss.victim
	pendingPath := m.pendingDataLoss.pendingOpenPath
	m.opentabs = m.opentabs.MarkCleanByID(victim.DocID)
	if m.store != nil && victim.DocID != 0 {
		_ = m.store.Bind(victim.DocID, victim.Path) // fire-and-forget: re-sync inode
		_ = m.store.MarkSaved(victim.DocID)         // fire-and-forget: §1.3
	}
	m.pendingDataLoss = pendingDataLoss{}
	m.opentabs = m.opentabs.CloseByID(victim.DocID)
	return m.requestOpenPath(0, pendingPath)
}
