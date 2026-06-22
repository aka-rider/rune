//go:build fuzzing

package workspace

import (
	"charm.land/bubbles/v2/key"
	tea "charm.land/bubbletea/v2"

	"rune/internal/fuzz/invariant"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/components/opentabs"
)

// FuzzInspect returns a read-only snapshot of model state for invariant checking.
// Called by the driver after every settled message.
// Value receiver — pure read, no side effects on m.
func (m Model) FuzzInspect() invariant.Snapshot {
	tabs, tabActive, hasDirty, activeTabDirty := tabsInfo(m)

	return invariant.Snapshot{
		// Editor content / cells
		Content:       m.editor.Content(),
		Cells:         m.editor.FuzzCells(),
		CursorOffsets: m.editor.CursorOffsets(),
		Focused:       m.editor.Focused(),

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
		ActiveFilePath: m.filePath,
		EditorPath:     m.filePath,
		DocID:          m.docID,
		FlushGen:       m.flushGen,
		SaveSnapshot:   m.activeSave.SavedContent,
		SaveInFlight:   m.activeSave.InFlight,

		// Layout — Frame is expensive; driver sets Width/Height from its own params
		Width:  m.totalWidth,
		Height: m.totalHeight,

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
func tabsInfo(m Model) ([]invariant.TabInfo, []bool, bool, bool) {
	raw := m.opentabs.FuzzTabs()
	activeHandle := m.opentabs.ActiveHandle()
	tabs := make([]invariant.TabInfo, len(raw))
	active := make([]bool, len(raw))
	hasDirty := false
	activeTabDirty := false
	for i, t := range raw {
		tabs[i] = invariant.TabInfo{Path: t.Path, Name: t.Name}
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
