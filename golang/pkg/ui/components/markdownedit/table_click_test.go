package markdownedit

import (
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/editor/cursor"
	"rune/pkg/terminal"
)

// tableDoc is a bordered grid table (4 model lines: header, separator, 2 body
// rows) followed by two plain text lines "X" and "Y" (model lines 4 and 5).
// With borders inserted, it renders as 9 display rows: top border, header,
// real separator, body1, inter-body border, body2, bottom border, X, Y.
const tableDoc = "| A | B |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\nX\nY"

// yLineOffset is "Y"'s buffer offset (the last line) — used to move the
// cursor off the table block so it renders formatted (grid layout with
// borders) rather than revealed source, matching how a real document looks
// once the user isn't actively editing the table.
const yLineOffset = 42

func newTableClickModel(t *testing.T) Model {
	t.Helper()
	m := newImagePipelineModel(t, terminal.TermCaps{})
	m, _ = m.SetContent(tableDoc)
	m = m.SetCursors([]cursor.Cursor{{Position: yLineOffset, Anchor: yLineOffset, ID: 1}})
	return m
}

// TestClickAfterTableResolvesCorrectLine locks in the fix for the
// wrap-space/display-space row confusion in DisplayToBuffer (TODO.md):
// once a bordered table has inflated m.snapshot.TotalRows past
// m.wrapSnap.TotalRows, clicking a text line after the table must resolve to
// that line, not an off-by-one neighbor. Pre-fix, clicking display row 7
// ("X") resolved to model line 5 ("Y") instead of model line 4 ("X") — a
// genuine wrong-line regression, verified by hand against the old clamp
// arithmetic (wrapRow = 7, clamped to wrapSnap.TotalRows-1 = 5).
func TestClickAfterTableResolvesCorrectLine(t *testing.T) {
	m := newTableClickModel(t)

	snap := m.Model.Geom().Snap
	if snap.TotalRows != 9 {
		t.Fatalf("setup: TotalRows=%d, want 9 (top border, header, sep, body1, inter-border, body2, bottom border, X, Y)", snap.TotalRows)
	}

	// Display row 7 is "X" (model line 4).
	m, _ = m.Update(tea.MouseClickMsg{X: 0, Y: 7, Button: tea.MouseLeft})
	if line := m.Model.OffsetToLineCol(m.Model.CursorOffset()).Line; line != 4 {
		t.Fatalf("click on display row 7 (\"X\"): cursor landed on model line %d, want 4", line)
	}
}

// TestClickOnTableBorderIsNoop locks in the UX decision that clicking a
// decorative table border/separator row does nothing, mirroring the existing
// image-reserved-row precedent, rather than snapping the cursor to a
// neighboring row.
func TestClickOnTableBorderIsNoop(t *testing.T) {
	m := newTableClickModel(t)
	// Position the caret somewhere known first.
	m, _ = m.Update(tea.MouseClickMsg{X: 0, Y: 7, Button: tea.MouseLeft}) // "X"
	before := m.Model.CursorOffset()

	for _, row := range []int{0, 4, 6} { // top border, inter-body border, bottom border
		m2, _ := m.Update(tea.MouseClickMsg{X: 0, Y: row, Button: tea.MouseLeft})
		if got := m2.Model.CursorOffset(); got != before {
			t.Errorf("click on border display row %d moved cursor from %d to %d, want no-op", row, before, got)
		}
	}
}

// TestDragThroughTableBorderDoesNotCorruptSelection locks in the
// handleMouseMotion gap closure: dragging across a table border row must not
// resolve DisplayToBuffer against a decorative row (no wrap-segment backing
// distinct from its neighbor), it must simply hold the selection at its last
// valid endpoint for that event.
func TestDragThroughTableBorderDoesNotCorruptSelection(t *testing.T) {
	m := newTableClickModel(t)

	// Click above the table (there is no line above it, so click on the top
	// border itself is a no-op per the guard) — start the drag from "X" instead
	// and drag upward through the bottom border onto body2.
	m, _ = m.Update(tea.MouseClickMsg{X: 0, Y: 7, Button: tea.MouseLeft}) // "X", model line 4
	anchorOffset := m.Model.CursorOffset()

	// Motion onto the bottom border (display row 6): must not move the
	// selection endpoint at all.
	m, _ = m.Update(tea.MouseMotionMsg{X: 0, Y: 6, Button: tea.MouseLeft})
	if got := m.Model.CursorOffset(); got != anchorOffset {
		t.Fatalf("motion onto border row: cursor moved from %d to %d, want held at anchor", anchorOffset, got)
	}

	// Motion onto body2 (display row 5, model line 3): selection must extend
	// normally once off the decorative row.
	m, _ = m.Update(tea.MouseMotionMsg{X: 0, Y: 5, Button: tea.MouseLeft})
	if line := m.Model.OffsetToLineCol(m.Model.CursorOffset()).Line; line != 3 {
		t.Fatalf("motion onto body2 (display row 5): selection endpoint on model line %d, want 3", line)
	}
}
