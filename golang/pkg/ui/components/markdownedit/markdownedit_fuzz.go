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

// FuzzBufferVersion forwards to the embedded textedit.Model.
func (m Model) FuzzBufferVersion() uint64 { return m.Model.FuzzBufferVersion() }

// FuzzLineCount forwards to the embedded textedit.Model.
func (m Model) FuzzLineCount() int { return m.Model.FuzzLineCount() }

// FuzzEditorWidth forwards to the embedded textedit.Model.
func (m Model) FuzzEditorWidth() int { return m.Model.FuzzEditorWidth() }

// FuzzWrapSnapshot forwards to the embedded textedit.Model.
func (m Model) FuzzWrapSnapshot() display.WrapSnapshot { return m.Model.FuzzWrapSnapshot() }

// FuzzSyntaxSnapshot forwards to the embedded textedit.Model.
func (m Model) FuzzSyntaxSnapshot() display.SyntaxSnapshot { return m.Model.FuzzSyntaxSnapshot() }
