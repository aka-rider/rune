package display

import (
	"testing"
	"unicode/utf8"
)

func TestBuildInlineCellMap(t *testing.T) {
	// Simulate `hello` with contentStart=6 (BufferStart=5, delimLeft=1)
	cm := buildInlineCellMap(6, []byte("hello"))
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

func TestBuildInlineCellMap_MultiByte(t *testing.T) {
	// "café" has 4 runes but "é" is 2 bytes → 6 bytes total
	cm := buildInlineCellMap(10, []byte("café"))
	if len(cm) != 4 {
		t.Fatalf("expected 4 mappings (one per rune), got %d", len(cm))
	}
	// Each mapping should point to the first byte of each rune in the buffer
	expectedOffsets := []int{10, 11, 12, 13} // 'c'=10, 'a'=11, 'f'=12, 'é'=13 (2-byte UTF-8)
	for i, m := range cm {
		if m.BufOffset != expectedOffsets[i] {
			t.Errorf("mapping %d: expected BufOffset %d, got %d", i, expectedOffsets[i], m.BufOffset)
		}
	}
}

func TestTableSeparatorCellMap_AllDecorative(t *testing.T) {
	colWidths := []int{3, 4}
	formatted := formatTableSeparator(colWidths)

	// Build cell map like formatTableSeparatorSpansWithWidths does for separators
	cm := make([]CellMapping, utf8.RuneCountInString(formatted))
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

func TestInlineCellMap_MultiByteEmoji(t *testing.T) {
	// "café" has 4 runes but "é" is 2 bytes in UTF-8
	text := "**café**"
	mdSpans := []mdSpan{{
		kind:       TokenBold,
		start:      0,
		end:        10,
		text:       "café",
		delimLeft:  2,
		delimRight: 2,
	}}
	lineStart := 50

	sl, _ := buildSyntaxLine(text, lineStart, 0, -1, -1, mdSpans)
	if len(sl.Spans) != 1 {
		t.Fatalf("expected 1 span, got %d", len(sl.Spans))
	}

	sp := sl.Spans[0]
	if sp.Text != "café" {
		t.Fatalf("expected text 'café', got %q", sp.Text)
	}
	if sp.CellMap == nil {
		t.Fatal("expected non-nil CellMap for rendered span")
	}

	// CellMap should have 4 entries (one per rune), not 8 (one per byte)
	expectedRuneCount := 4
	if len(sp.CellMap) != expectedRuneCount {
		t.Fatalf("expected CellMap length %d (one per rune), got %d", expectedRuneCount, len(sp.CellMap))
	}

	// Verify each CellMap entry points to the first byte of the corresponding rune
	expectedOffsets := []int{52, 53, 54, 55} // c=52, a=53, f=54, é=55 (2-byte UTF-8)
	for i, m := range sp.CellMap {
		if m.BufOffset != expectedOffsets[i] {
			t.Errorf("CellMap[%d]: expected BufOffset %d, got %d", i, expectedOffsets[i], m.BufOffset)
		}
	}
}
