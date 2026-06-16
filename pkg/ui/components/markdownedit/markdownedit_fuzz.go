//go:build fuzzing

package markdownedit

import (
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/display"
	"rune/pkg/ui/components/textedit"
)

// FuzzCells forwards to the embedded textedit.Model.FuzzCells().
func (m Model) FuzzCells() [][]textedit.Cell {
	return m.Model.FuzzCells()
}

// FuzzSnapshot forwards to the embedded textedit.Model.FuzzSnapshot().
func (m Model) FuzzSnapshot() display.DisplaySnapshot {
	return m.Model.FuzzSnapshot()
}

// FuzzCursors forwards to the embedded textedit.Model.FuzzCursors().
func (m Model) FuzzCursors() []cursor.Cursor {
	return m.Model.FuzzCursors()
}
