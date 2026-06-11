package editor

import (
	"testing"

	"charm.land/lipgloss/v2"

	"rune/pkg/editor/display"
)

func TestSpanToCells_RevealedSpan(t *testing.T) {
	sp := display.DisplaySpan{
		Text:        "hello",
		State:       display.Revealed,
		BufferStart: 10,
		BufferEnd:   15,
	}
	cells := spanToCells(sp, lipgloss.NewStyle())
	if len(cells) != 5 {
		t.Fatalf("expected 5 cells, got %d", len(cells))
	}
	for i, c := range cells {
		if c.BufOffset != 10+i {
			t.Errorf("cell %d: expected BufOffset %d, got %d", i, 10+i, c.BufOffset)
		}
		if c.Width != 1 {
			t.Errorf("cell %d: expected Width 1, got %d", i, c.Width)
		}
	}
	// Verify rune content
	if cells[0].Rune != 'h' || cells[4].Rune != 'o' {
		t.Errorf("unexpected rune content: first=%c last=%c", cells[0].Rune, cells[4].Rune)
	}
}

func TestSpanToCells_RenderedSpanWithCellMap(t *testing.T) {
	// Simulate inline code `hello` where BufferStart=5 (backtick), delimLeft=1
	// CellMap maps each visible byte to buffer offset after the delimiter
	cm := []display.CellMapping{
		{BufOffset: 6}, // 'h' at buffer offset 6 (5 + 1 delim)
		{BufOffset: 7}, // 'e'
		{BufOffset: 8}, // 'l'
		{BufOffset: 9}, // 'l'
		{BufOffset: 10}, // 'o'
	}
	sp := display.DisplaySpan{
		Text:        "hello",
		Kind:        display.TokenInlineCode,
		State:       display.Rendered,
		BufferStart: 5,
		BufferEnd:   12,
		CellMap:     cm,
	}
	cells := spanToCells(sp, lipgloss.NewStyle())
	if len(cells) != 5 {
		t.Fatalf("expected 5 cells, got %d", len(cells))
	}
	for i, c := range cells {
		expected := 6 + i
		if c.BufOffset != expected {
			t.Errorf("cell %d: expected BufOffset %d, got %d", i, expected, c.BufOffset)
		}
	}
}

func TestSpanToCells_TableSpanDecorativeCells(t *testing.T) {
	// Table span with padding cells mapped to -1
	cm := []display.CellMapping{
		{BufOffset: -1},  // '|'
		{BufOffset: -1},  // ' '
		{BufOffset: 10},  // 'a'
		{BufOffset: 11},  // 'b'
		{BufOffset: -1},  // ' ' (padding)
		{BufOffset: -1},  // ' '
		{BufOffset: -1},  // '|'
	}
	sp := display.DisplaySpan{
		Text:        "| ab  |",
		Kind:        display.TokenTable,
		State:       display.Rendered,
		BufferStart: 0,
		BufferEnd:   5,
		CellMap:     cm,
	}
	cells := spanToCells(sp, lipgloss.NewStyle())
	if len(cells) != 7 {
		t.Fatalf("expected 7 cells, got %d", len(cells))
	}
	if cells[0].BufOffset != -1 {
		t.Errorf("cell 0 (pipe) should be decorative (-1), got %d", cells[0].BufOffset)
	}
	if cells[2].BufOffset != 10 {
		t.Errorf("cell 2 ('a') expected BufOffset 10, got %d", cells[2].BufOffset)
	}
	if cells[4].BufOffset != -1 {
		t.Errorf("cell 4 (padding) should be decorative (-1), got %d", cells[4].BufOffset)
	}
}

func TestApplyOverlays_Selection(t *testing.T) {
	cells := []Cell{
		{Rune: 'a', BufOffset: 5},
		{Rune: 'b', BufOffset: 6},
		{Rune: 'c', BufOffset: 7},
		{Rune: 'd', BufOffset: -1}, // decorative
		{Rune: 'e', BufOffset: 8},
	}
	selections := []selInterval{{start: 6, end: 8}} // selects 'b' and 'c'
	cursorOffsets := map[int]bool{8: true}           // cursor on 'e'

	applyOverlays(cells, cursorOffsets, selections)

	if cells[0].Selected || cells[0].Cursor {
		t.Error("cell 0 should not be selected or cursor")
	}
	if !cells[1].Selected {
		t.Error("cell 1 ('b') should be selected")
	}
	if !cells[2].Selected {
		t.Error("cell 2 ('c') should be selected")
	}
	if cells[3].Selected || cells[3].Cursor {
		t.Error("cell 3 (decorative) should not be selected or cursor")
	}
	if !cells[4].Cursor {
		t.Error("cell 4 ('e') should have cursor")
	}
}

func TestSliceCells_BasicHorizontalScroll(t *testing.T) {
	cells := make([]Cell, 10)
	for i := range cells {
		cells[i] = Cell{Rune: rune('a' + i), Width: 1, BufOffset: i}
	}

	// Scroll to col 3, view width 4: should show 'd', 'e', 'f', 'g'
	result := sliceCells(cells, 3, 4)
	if len(result) != 4 {
		t.Fatalf("expected 4 cells, got %d", len(result))
	}
	if result[0].Rune != 'd' || result[3].Rune != 'g' {
		t.Errorf("expected [d,e,f,g], got [%c,%c,%c,%c]",
			result[0].Rune, result[1].Rune, result[2].Rune, result[3].Rune)
	}
	// BufOffset should be preserved
	if result[0].BufOffset != 3 {
		t.Errorf("expected BufOffset 3, got %d", result[0].BufOffset)
	}
}

func TestSliceCells_WideCharAtLeftEdge(t *testing.T) {
	cells := []Cell{
		{Rune: 'a', Width: 1, BufOffset: 0},
		{Rune: '中', Width: 2, BufOffset: 1}, // wide char at col 1-2
		{Rune: 'b', Width: 1, BufOffset: 4},
		{Rune: 'c', Width: 1, BufOffset: 5},
	}

	// Scroll to col 2: the wide char starts at col 1, extends to col 2
	// It partially overlaps — should be replaced with padding
	result := sliceCells(cells, 2, 4)
	if len(result) < 1 {
		t.Fatal("expected at least 1 cell")
	}
	// First cell should be padding (space) since '中' is partially cut
	if result[0].Rune != ' ' {
		t.Errorf("expected padding space, got %c", result[0].Rune)
	}
	if result[0].BufOffset != -1 {
		t.Errorf("padding cell should have BufOffset -1, got %d", result[0].BufOffset)
	}
}

func TestCellsToString_NoOverlays(t *testing.T) {
	cells := []Cell{
		{Rune: 'h', Width: 1, Style: lipgloss.NewStyle()},
		{Rune: 'i', Width: 1, Style: lipgloss.NewStyle()},
	}
	selStyle := lipgloss.NewStyle().Background(lipgloss.Color("1"))
	cursorStyle := lipgloss.NewStyle().Reverse(true)

	result := cellsToString(cells, selStyle, cursorStyle)
	// With no overlays and default style, should produce plain text
	if result != "hi" {
		t.Errorf("expected 'hi', got %q", result)
	}
}

func TestCellsToString_WithCursor(t *testing.T) {
	cursorStyle := lipgloss.NewStyle().Reverse(true)
	selStyle := lipgloss.NewStyle().Background(lipgloss.Color("1"))

	cells := []Cell{
		{Rune: 'a', Width: 1, Style: lipgloss.NewStyle()},
		{Rune: 'b', Width: 1, Style: lipgloss.NewStyle(), Cursor: true},
		{Rune: 'c', Width: 1, Style: lipgloss.NewStyle()},
	}

	result := cellsToString(cells, selStyle, cursorStyle)
	// The cursor cell should be rendered with reverse
	expected := "a" + cursorStyle.Render("b") + "c"
	if result != expected {
		t.Errorf("expected %q, got %q", expected, result)
	}
}

// ============================================================================
// Specification: stylesEqual must distinguish styles that render differently
// ============================================================================

func TestStylesEqual_LinkVsPlain(t *testing.T) {
	link := lipgloss.NewStyle().Foreground(lipgloss.Color("4")).Underline(true)
	plain := lipgloss.NewStyle()
	if stylesEqual(link, plain) {
		t.Error("stylesEqual(Link, Plain) = true, want false — link style differs from plain")
	}
}

func TestStylesEqual_DifferentBackgrounds(t *testing.T) {
	a := lipgloss.NewStyle().Background(lipgloss.Color("1"))
	b := lipgloss.NewStyle()
	if stylesEqual(a, b) {
		t.Error("stylesEqual(background(1), plain) = true, want false")
	}
}

func TestStylesEqual_Reflexive(t *testing.T) {
	a := lipgloss.NewStyle().Foreground(lipgloss.Color("4")).Underline(true)
	if !stylesEqual(a, a) {
		t.Error("stylesEqual(a, a) = false, want true — same style must be equal")
	}
}

func TestCellsToString_StyleDoesNotBleed(t *testing.T) {
	selStyle := lipgloss.NewStyle()
	cursorStyle := lipgloss.NewStyle()
	link := lipgloss.NewStyle().Foreground(lipgloss.Color("4")).Underline(true)
	plain := lipgloss.NewStyle()

	cells := []Cell{
		{Rune: 'a', Width: 1, Style: plain},
		{Rune: ' ', Width: 1, Style: plain},
		{Rune: 'b', Width: 1, Style: link},
		{Rune: 'c', Width: 1, Style: plain},
	}
	result := cellsToString(cells, selStyle, cursorStyle)

	want := "a " + link.Render("b") + "c"
	if result != want {
		t.Errorf("cellsToString produced unexpected output\ngot:  %q\nwant: %q", result, want)
	}
}

func TestCellsToString_LinkFormattingPreserved(t *testing.T) {
	selStyle := lipgloss.NewStyle()
	cursorStyle := lipgloss.NewStyle()
	link := lipgloss.NewStyle().Foreground(lipgloss.Color("4")).Underline(true)
	plain := lipgloss.NewStyle()

	cells := []Cell{
		{Rune: 't', Width: 1, Style: plain},
		{Rune: 'e', Width: 1, Style: plain},
		{Rune: 'x', Width: 1, Style: plain},
		{Rune: 't', Width: 1, Style: plain},
		{Rune: ' ', Width: 1, Style: plain},
		{Rune: 'l', Width: 1, Style: link},
		{Rune: 'i', Width: 1, Style: link},
		{Rune: 'n', Width: 1, Style: link},
		{Rune: 'k', Width: 1, Style: link},
		{Rune: ' ', Width: 1, Style: plain},
		{Rune: 'e', Width: 1, Style: plain},
		{Rune: 'n', Width: 1, Style: plain},
		{Rune: 'd', Width: 1, Style: plain},
	}
	result := cellsToString(cells, selStyle, cursorStyle)

	want := "text " + link.Render("link") + " end"
	if result != want {
		t.Errorf("cellsToString dropped link formatting\ngot:  %q\nwant: %q", result, want)
	}
}

// ============================================================================
// Specification: spanToCells dispatch correctness
// ============================================================================

func TestSpanToCells_RevealedSpanUsesBufferStart(t *testing.T) {
	sp := display.DisplaySpan{
		Text:        "hi",
		State:       display.Revealed,
		BufferStart: 10,
		BufferEnd:   12,
	}
	cells := spanToCells(sp, lipgloss.NewStyle())
	if len(cells) != 2 {
		t.Fatalf("expected 2 cells, got %d", len(cells))
	}
	if cells[0].BufOffset != 10 {
		t.Errorf("cell 0: expected BufOffset 10, got %d", cells[0].BufOffset)
	}
	if cells[1].BufOffset != 11 {
		t.Errorf("cell 1: expected BufOffset 11, got %d", cells[1].BufOffset)
	}
}

func TestSpanToCells_RenderedWithCellMap(t *testing.T) {
	cm := []display.CellMapping{
		{BufOffset: 8},
		{BufOffset: 9},
	}
	sp := display.DisplaySpan{
		Text:        "ab",
		Kind:        display.TokenLink,
		State:       display.Rendered,
		BufferStart: 5,
		BufferEnd:   15,
		CellMap:     cm,
	}
	cells := spanToCells(sp, lipgloss.NewStyle())
	if len(cells) != 2 {
		t.Fatalf("expected 2 cells, got %d", len(cells))
	}
	// BufOffset must come from CellMap, NOT from BufferStart + pos
	if cells[0].BufOffset != 8 {
		t.Errorf("cell 0: expected BufOffset 8 (from CellMap), got %d — must not use BufferStart+pos",
			cells[0].BufOffset)
	}
	if cells[1].BufOffset != 9 {
		t.Errorf("cell 1: expected BufOffset 9 (from CellMap), got %d — must not use BufferStart+pos",
			cells[1].BufOffset)
	}
}

func TestSpanToCells_RenderedNilCellMap(t *testing.T) {
	sp := display.DisplaySpan{
		Text:        "hi",
		State:       display.Rendered,
		BufferStart: 10,
		BufferEnd:   12,
		CellMap:     nil,
	}
	cells := spanToCells(sp, lipgloss.NewStyle())
	if len(cells) != 2 {
		t.Fatalf("expected 2 cells, got %d", len(cells))
	}
	// Rendered span with nil CellMap (code fence, frontmatter) falls back to BufferStart + pos
	if cells[0].BufOffset != 10 {
		t.Errorf("cell 0: expected BufOffset 10 (fallback), got %d", cells[0].BufOffset)
	}
	if cells[1].BufOffset != 11 {
		t.Errorf("cell 1: expected BufOffset 11 (fallback), got %d", cells[1].BufOffset)
	}
}
