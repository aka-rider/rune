package textedit_test

import (
	"testing"

	"charm.land/lipgloss/v2"

	"rune/pkg/editor/display"
	"rune/pkg/ui/components/textedit"
)

// TestSpanToCells_NewlineSkipped verifies that a \n character inside span
// text is silently dropped and never becomes a width-1 cell. A cell with
// Rune == '\n' would cause CellsToString to emit a terminal newline, adding
// a phantom row that corrupts the frame height.
func TestSpanToCells_NewlineSkipped(t *testing.T) {
	sp := display.DisplaySpan{
		Text:        "hello\n",
		State:       display.Revealed,
		BufferStart: 0,
	}
	cells := textedit.SpanToCells(sp, lipgloss.NewStyle())
	for _, c := range cells {
		if c.Rune == '\n' {
			t.Errorf("SpanToCells produced a cell with Rune = '\\n'; should be skipped")
		}
	}
	if len(cells) != 5 {
		t.Errorf("cell count = %d, want 5 (\\n must be skipped, only 'hello' = 5 cells)", len(cells))
	}
}
