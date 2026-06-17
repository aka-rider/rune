//go:build fuzzing

package workspace

import (
	"rune/internal/fuzz/invariant"
	"rune/pkg/ui/components/footer"
)

// FuzzInspect returns a read-only snapshot of model state for invariant checking.
// Called by the driver after every settled message.
// Value receiver — pure read, no side effects on m.
func (m Model) FuzzInspect() invariant.Snapshot {
	tabs, tabActive, hasDirty := tabsInfo(m)

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

// tabsInfo builds the Tabs/TabActive/HasDirtyFile triple from opentabs.
func tabsInfo(m Model) ([]invariant.TabInfo, []bool, bool) {
	raw := m.opentabs.FuzzTabs()
	tabs := make([]invariant.TabInfo, len(raw))
	active := make([]bool, len(raw))
	hasDirty := false
	for i, t := range raw {
		tabs[i] = invariant.TabInfo{Path: t.Path, Name: t.Name}
		active[i] = t.Active
		if t.Dirty {
			hasDirty = true
		}
	}
	return tabs, active, hasDirty
}
