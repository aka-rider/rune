package textedit

import (
	"rune/pkg/editor/buffer"
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
	return m.renderCells(m.contentHeight(), nil, nil)
}

// FuzzSnapshot returns the current display snapshot.
func (m Model) FuzzSnapshot() display.DisplaySnapshot {
	return m.snapshot
}

// FuzzCursors returns all active cursors.
func (m Model) FuzzCursors() []cursor.Cursor {
	return m.cursors.All()
}

// FuzzBufferVersion returns the buffer's monotone version counter.
func (m Model) FuzzBufferVersion() uint64 { return m.buf.Version() }

// FuzzLineCount returns the buffer's line count (number of '\n' + 1).
func (m Model) FuzzLineCount() int { return m.buf.LineCount() }

// FuzzEditorWidth returns the textedit model's current width (0 = unset/unwrapped).
func (m Model) FuzzEditorWidth() int { return m.width }

// FuzzWrapSnapshot returns the current wrap-layer snapshot.
// Used for WRAP-RT, COORD-RT, and SPAN-COVER invariants.
func (m Model) FuzzWrapSnapshot() display.WrapSnapshot { return m.wrapSnap }

// FuzzSyntaxSnapshot returns the current syntax-layer snapshot.
// Used for COORD-RT, SPAN-COVER, and D5 invariants.
func (m Model) FuzzSyntaxSnapshot() display.SyntaxSnapshot { return m.syntaxSnap }

// FuzzLastEdits returns the CursorID-tagged, pre-edit-coordinate edits
// applied by the message just processed (nil if that message produced no
// cursor-driven edit — see Model.lastEdits). Used by SEL-EDIT to verify a
// selecting cursor's actual edit range matched its own selection.
func (m Model) FuzzLastEdits() []buffer.Edit { return m.lastEdits }
