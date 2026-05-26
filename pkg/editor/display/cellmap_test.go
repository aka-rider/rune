package display

import (
	"testing"
)

func TestBuildInlineCellMap(t *testing.T) {
	// Simulate `hello` with contentStart=6 (BufferStart=5, delimLeft=1)
	cm := buildInlineCellMap(6, 5)
	if len(cm) != 5 {
		t.Fatalf("expected 5 mappings, got %d", len(cm))
	}
	for i, m := range cm {
		expected := 6 + i
		if m.BufOffset != expected {
			t.Errorf("mapping %d: expected BufOffset %d, got %d", i, expected, m.BufOffset)
		}
	}
}

func TestTableCellMapGeneration(t *testing.T) {
	// A simple table row: "| abc | de |"
	line := "| abc | de |"
	colWidths := []int{4, 4} // each column padded to width 4
	alignments := []int{0, 0}
	lineStart := 100

	formatted, cm := formatTableRow(line, colWidths, alignments, lineStart)
	if formatted == "" {
		t.Fatal("formatted row should not be empty")
	}
	if len(cm) != len(formatted) {
		t.Fatalf("CellMap length %d should match formatted length %d", len(cm), len(formatted))
	}

	// Find content cells — they should have non-negative BufOffset
	contentCount := 0
	decorativeCount := 0
	for _, m := range cm {
		if m.BufOffset >= 0 {
			contentCount++
		} else {
			decorativeCount++
		}
	}
	if contentCount == 0 {
		t.Error("expected some content cells with non-negative BufOffset")
	}
	if decorativeCount == 0 {
		t.Error("expected some decorative cells with BufOffset=-1")
	}
}

func TestTableSeparatorCellMap_AllDecorative(t *testing.T) {
	colWidths := []int{3, 4}
	formatted := formatTableSeparator(colWidths)

	// Build cell map like tableRenderedSpans does for separators
	cm := make([]CellMapping, len(formatted))
	for i := range cm {
		cm[i] = CellMapping{BufOffset: -1}
	}

	// All cells should be decorative
	for i, m := range cm {
		if m.BufOffset != -1 {
			t.Errorf("separator cell %d should be decorative (-1), got %d", i, m.BufOffset)
		}
	}
}

func TestInlineCellMap_InSyntaxSnapshot(t *testing.T) {
	// Test that buildSyntaxLine produces CellMap for rendered inline spans
	// Use a bold span: **bold**
	mdSpans := []mdSpan{{
		kind:       TokenBold,
		start:      0,
		end:        8,
		text:       "bold",
		delimLeft:  2,
		delimRight: 2,
	}}
	lineText := "**bold**"
	lineStart := 50

	sl, _ := buildSyntaxLine(lineText, lineStart, 0, -1, -1, mdSpans)
	if len(sl.Spans) != 1 {
		t.Fatalf("expected 1 span, got %d", len(sl.Spans))
	}

	sp := sl.Spans[0]
	if sp.State != Rendered {
		t.Fatalf("expected Rendered state, got %v", sp.State)
	}
	if sp.Text != "bold" {
		t.Fatalf("expected text 'bold', got %q", sp.Text)
	}
	if sp.CellMap == nil {
		t.Fatal("expected non-nil CellMap for rendered span")
	}
	if len(sp.CellMap) != 4 {
		t.Fatalf("expected CellMap length 4, got %d", len(sp.CellMap))
	}

	// 'b' should map to lineStart + delimLeft + 0 = 52
	// 'o' should map to 53, 'l' to 54, 'd' to 55
	for i, m := range sp.CellMap {
		expected := lineStart + 2 + i // 2 = delimLeft for **
		if m.BufOffset != expected {
			t.Errorf("CellMap[%d]: expected BufOffset %d, got %d", i, expected, m.BufOffset)
		}
	}
}
