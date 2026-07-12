package workspace

import (
	"charm.land/bubbles/v2/key"
	tea "charm.land/bubbletea/v2"

	"rune/internal/fuzz/snapshot"
	"rune/pkg/ui/pages/workspace/mergemode"
)

// FuzzInspect returns a read-only snapshot of model state for invariant checking.
// Called by the driver after every settled message.
// Value receiver — pure read, no side effects on m.
//
// The editor/tabs/footer-derived fields come from snapshot.FromTextedit
// (via markdownedit's embedded textedit.Model)/FromOpenTabs/FromFooter — the
// SAME builder functions internal/fuzz/invarianttest's CheckTextedit/
// CheckOpenTabs/CheckFooter use for a standalone component under test.
// Exactly one field mapping; the workspace-only fields (file/persistence,
// merge, layout, filetree, ...) are layered on top here, since only the
// full workspace has that context.
func (m Model) FuzzInspect() snapshot.Snapshot {
	s := snapshot.FromMarkdownedit(m.editor)
	tabs := snapshot.FromOpenTabs(m.opentabs)
	guard := snapshot.FromFooter(m.footer)

	s.Tabs = tabs.Tabs
	s.ActiveTabIdx = tabs.ActiveTabIdx // same source (opentabs.Cursor) — keep ONE mapping, the builder.
	s.TabActive = tabs.TabActive
	s.TabCount = tabs.TabCount
	s.TabLimit = tabLimit
	s.HasDirtyFile = tabs.HasDirtyFile
	s.ActiveTabDirty = tabs.ActiveTabDirty

	s.GuardVisible = guard.GuardVisible
	s.GuardKind = guard.GuardKind
	s.GuardOptionCount = guard.GuardOptionCount
	s.ChordPending = guard.ChordPending
	// StoreDegraded is guard.StoreDegraded (m.footer.Degraded()) — assigned
	// below alongside the other file/persistence fields for readability.

	// File / persistence
	s.ActiveFilePath = m.view.Path()
	s.EditorPath = m.view.Path()
	s.DocID = m.view.DocID()
	s.Loading = m.pendingLoad.active
	s.FlushGen = m.flushGen
	s.SaveSnapshot = m.activeSave.SavedContent
	s.SaveInFlight = m.activeSave.InFlight
	s.PendingDataLossKind = int(m.pendingDataLoss.kind) // mirrors actionKind iota order
	s.SaveRequestID = m.activeSave.RequestID

	s.PendingReopenActive = m.pendingReopen.active
	s.PendingConflictActive = m.pendingConflict.active
	s.PendingDeletedActive = m.pendingDeleted.active
	s.PendingRacedActive = m.pendingRaced.active
	s.StoreDegraded = guard.StoreDegraded

	s.MergeActive = mergemode.IsActive(m.merge)
	s.MergeUnresolved = mergemode.HasUnresolvedConflicts(m.merge)

	// Layout — Frame is set by driver; driver also sets Width/Height
	s.Width = m.totalWidth
	s.Height = m.totalHeight

	// Guard / chord / focus
	s.FocusPane = int(m.focus)

	// Filetree
	s.FiletreeCursor = m.filetree.FuzzCursor()
	s.FiletreeLen = m.filetree.FuzzLen()

	return s
}

// IsCloseFileMsg reports whether msg is a CloseFile (^w) key press.
// Used by the fuzz driver to annotate snapshots for the G3 invariant.
func (m Model) IsCloseFileMsg(msg tea.Msg) bool {
	kp, ok := msg.(tea.KeyPressMsg)
	if !ok {
		return false
	}
	return key.Matches(kp, m.keys.CloseFile)
}

// IsUndoRedoMsg reports whether msg is an Undo (⌘Z) or Redo (^Y) key press —
// the same key.Matches checks handleKeyPress's Priority 2.5 branch uses.
// REDO-CLEAR (WP2) excludes these from "a buffer-changing key press must
// truncate the redo future": undo/redo themselves move the journal position,
// they don't append a fresh edit that abandons one.
func (m Model) IsUndoRedoMsg(msg tea.Msg) bool {
	kp, ok := msg.(tea.KeyPressMsg)
	if !ok {
		return false
	}
	return key.Matches(kp, m.keys.Undo) || key.Matches(kp, m.keys.Redo)
}
