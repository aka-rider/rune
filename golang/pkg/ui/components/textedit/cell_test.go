package textedit

import (
	"testing"

	"charm.land/lipgloss/v2"

	"rune/internal/editortest"
	"rune/pkg/editor/display"
)

// TestSpanToCells_NewlineSkipped verifies that a \n character inside span
// text is silently dropped and never becomes a width-1 cell. A cell with
// Rune == '\n' would cause cellsToString to emit a terminal newline, adding
// a phantom row that corrupts the frame height.
func TestSpanToCells_NewlineSkipped(t *testing.T) {
	sp := display.DisplaySpan{
		Text:        "hello\n",
		State:       display.Revealed,
		BufferStart: 0,
	}
	cells := SpanToCells(sp, lipgloss.NewStyle())
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
//
// D5: package textedit (internal), not textedit_test — cellsToString has no
// caller outside this package and was unexported.
func TestCellsToString_DimPerRun(t *testing.T) {
	plain := lipgloss.NewStyle()
	link := lipgloss.NewStyle().Underline(true)
	cells := []Cell{
		{Rune: 's', Width: 1, Style: plain, BufOffset: 0},
		{Rune: 'e', Width: 1, Style: plain, BufOffset: 1},
		{Rune: 'L', Width: 1, Style: link, BufOffset: 2},
		{Rune: 'K', Width: 1, Style: link, BufOffset: 3},
		{Rune: 'e', Width: 1, Style: plain, BufOffset: 4},
		{Rune: 'd', Width: 1, Style: plain, BufOffset: 5},
	}
	no := lipgloss.NewStyle()

	dimOut := cellsToString(cells, no, no, no, no, true)
	plainOut := cellsToString(cells, no, no, no, no, false)

	// Rendered oracle instead of counting raw "\x1b[2m" escapes: dim=true
	// must be byte-identical to pre-fainting EVERY cell's own style and
	// rendering normally — which is exactly what BUG2's fix guarantees and
	// what the old post-hoc Faint(true).Render(assembledANSI) could not
	// (the link run's inner reset cleared faint for the "ed" run after it).
	wantCells := make([]Cell, len(cells))
	for i, c := range cells {
		c.Style = c.Style.Faint(true)
		wantCells[i] = c
	}
	wantDim := cellsToString(wantCells, no, no, no, no, false)
	if dimOut != wantDim {
		t.Errorf("dim output != every-run-fainted oracle:\n got %q\nwant %q", dimOut, wantDim)
	}

	// Dimming must never alter the visible text itself.
	if got := editortest.StripANSI(dimOut); got != "seLKed" {
		t.Errorf("dim output text = %q, want %q", got, "seLKed")
	}
	if dimOut == plainOut {
		t.Error("dim and non-dim output are identical; dimming had no effect")
	}
}
