//go:build fuzzing

package textedit

import (
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/display"
)

// FuzzCells returns the cell grid currently rendered by View().
// Uses the same renderCells helper so the fuzzer checks exactly
// what the terminal renders — no drift from the real View().
func (m Model) FuzzCells() [][]Cell {
	if m.height == 0 {
		return nil
	}
	return m.renderCells(m.contentHeight())
}

// FuzzSnapshot returns the current display snapshot.
func (m Model) FuzzSnapshot() display.DisplaySnapshot {
	return m.snapshot
}

// FuzzCursors returns all active cursors.
func (m Model) FuzzCursors() []cursor.Cursor {
	return m.cursors.All()
}
