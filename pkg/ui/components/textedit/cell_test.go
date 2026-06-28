package textedit_test

import (
	"strings"
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

// TestCellsToString_DimPerRun verifies BUG2: when dimming an unfocused line,
// faint must apply to EVERY style run, not just the first. The old post-hoc
// Faint(true).Render(assembledANSI) left only the first run dim because the
// inner reset cleared faint. With per-run dimming, a plain run AFTER an
// intervening styled (link) run is still faint.
func TestCellsToString_DimPerRun(t *testing.T) {
	plain := lipgloss.NewStyle()
	link := lipgloss.NewStyle().Underline(true)
	cells := []textedit.Cell{
		{Rune: 's', Width: 1, Style: plain, BufOffset: 0},
		{Rune: 'e', Width: 1, Style: plain, BufOffset: 1},
		{Rune: 'L', Width: 1, Style: link, BufOffset: 2},
		{Rune: 'K', Width: 1, Style: link, BufOffset: 3},
		{Rune: 'e', Width: 1, Style: plain, BufOffset: 4},
		{Rune: 'd', Width: 1, Style: plain, BufOffset: 5},
	}
	no := lipgloss.NewStyle()

	const faint = "\x1b[2m"
	dimOut := textedit.CellsToString(cells, no, no, no, no, true)
	plainOut := textedit.CellsToString(cells, no, no, no, no, false)

	// Both plain runs ("se" before the link, "ed" after it) must carry faint —
	// proving faint survives past the link run's reset.
	if got := strings.Count(dimOut, faint); got < 2 {
		t.Errorf("dim output has %d faint markers, want >=2 (faint must apply to every run): %q", got, dimOut)
	}
	if strings.Contains(plainOut, faint) {
		t.Errorf("non-dim output must not contain faint: %q", plainOut)
	}
	if dimOut == plainOut {
		t.Error("dim and non-dim output are identical; dimming had no effect")
	}
}
