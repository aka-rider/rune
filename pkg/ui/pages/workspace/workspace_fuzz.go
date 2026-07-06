//go:build fuzzing

package workspace

import (
	"charm.land/bubbles/v2/key"
	tea "charm.land/bubbletea/v2"

	"rune/internal/fuzz/snapshot"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/components/opentabs"
	"rune/pkg/ui/pages/workspace/mergemode"
)

// FuzzInspect returns a read-only snapshot of model state for invariant checking.
// Called by the driver after every settled message.
// Value receiver — pure read, no side effects on m.
func (m Model) FuzzInspect() snapshot.Snapshot {
	tabs, tabActive, hasDirty, activeTabDirty := tabsInfo(m)

	return snapshot.Snapshot{
		// Editor content / cells
		Content:       m.editor.Content(),
		Cells:         m.editor.FuzzCells(),
		CursorOffsets: m.editor.CursorOffsets(),
		Focused:       m.editor.Focused(),
		ReadOnly:      m.editor.ReadOnly(),

		// Editor structural
		Cursors:       m.editor.FuzzCursors(),
		BufferVersion: m.editor.FuzzBufferVersion(),
		LineCount:     m.editor.FuzzLineCount(),

		// Display pipeline
		Display: m.editor.FuzzSnapshot(),
		Wrap:    m.editor.FuzzWrapSnapshot(),
		Syntax:  m.editor.FuzzSyntaxSnapshot(),

		// Tabs
		Tabs:         tabs,
		ActiveTabIdx: m.opentabs.Cursor(),
		TabActive:    tabActive,
		TabCount:     m.opentabs.Len(),
		TabLimit:     tabLimit,

		// File / persistence
		ActiveFilePath:      m.view.Path(),
		EditorPath:          m.view.Path(),
		DocID:               m.view.DocID(),
		Loading:             m.pendingLoad.active,
		FlushGen:            m.flushGen,
		SaveSnapshot:        m.activeSave.SavedContent,
		SaveInFlight:        m.activeSave.InFlight,
		PendingDataLossKind: int(m.pendingDataLoss.kind), // mirrors actionKind iota order
		SaveRequestID:       m.activeSave.RequestID,

		PendingConflictActive: m.pendingConflict.active,
		PendingDeletedActive:  m.pendingDeleted.active,
		PendingRacedActive:    m.pendingRaced.active,
		StoreDegraded:         m.footer.Degraded(),

		MergeActive:     mergemode.IsActive(m.merge),
		MergeUnresolved: mergemode.HasUnresolvedConflicts(m.merge),

		// Layout — Frame is set by driver; driver also sets Width/Height
		Width:       m.totalWidth,
		Height:      m.totalHeight,
		EditorWidth: m.editor.FuzzEditorWidth(),

		// Guard / chord / focus
		HasDirtyFile:     hasDirty,
		ActiveTabDirty:   activeTabDirty,
		GuardVisible:     m.footer.InGuard(),
		GuardKind:        footer.GuardKind(m.footer.GuardKind()),
		GuardOptionCount: m.footer.GuardOptionCount(),
		ChordPending:     m.footer.PendingKey() != "",
		FocusPane:        int(m.focus),

		// Filetree
		FiletreeCursor: m.filetree.FuzzCursor(),
		FiletreeLen:    m.filetree.FuzzLen(),
	}
}

// tabsInfo builds the Tabs/TabActive/HasDirtyFile/ActiveTabDirty tuple from opentabs.
func tabsInfo(m Model) ([]snapshot.TabInfo, []bool, bool, bool) {
	raw := m.opentabs.FuzzTabs()
	activeHandle := m.opentabs.ActiveHandle()
	tabs := make([]snapshot.TabInfo, len(raw))
	active := make([]bool, len(raw))
	hasDirty := false
	activeTabDirty := false
	for i, t := range raw {
		tabs[i] = snapshot.TabInfo{Path: t.Path, Name: t.Name, DocID: t.DocID}
		th := opentabs.TabHandle{DocID: t.DocID, Path: t.Path}
		isActive := th.Equal(activeHandle)
		active[i] = isActive
		if t.Dirty {
			hasDirty = true
			if isActive {
				activeTabDirty = true
			}
		}
	}
	return tabs, active, hasDirty, activeTabDirty
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
