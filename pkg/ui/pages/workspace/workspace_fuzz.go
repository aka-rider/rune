//go:build fuzzing

package workspace

import (
	"rune/internal/fuzz/invariant"
)

// FuzzInspect returns a read-only snapshot of model state for invariant checking.
// Called by the driver after every settled message.
// Value receiver — pure read, no side effects on m.
func (m Model) FuzzInspect() invariant.Snapshot {
	return invariant.Snapshot{
		Content:       m.editor.Content(),
		Cells:         m.editor.FuzzCells(),
		CursorOffsets: m.editor.CursorOffsets(),
		Focused:       m.editor.Focused(),
		Tabs:           tabsSnapshot(m),
		ActiveTabIdx:   m.opentabs.Cursor(),
		ActiveFilePath: m.filePath,
	}
}

func tabsSnapshot(m Model) []invariant.TabInfo {
	count := m.opentabs.Height() - 1
	if count < 0 {
		count = 0
	}
	tabs := make([]invariant.TabInfo, count)
	for i := 0; i < count; i++ {
		tabs[i] = invariant.TabInfo{Path: m.opentabs.PathAt(i)}
	}
	return tabs
}
