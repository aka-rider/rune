package display_test

import (
	"strings"
	"testing"
	"unicode/utf8"

	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/display"
)

// TestTableBoldTextNoTruncation reproduces the "iide" vs "inside" bug.

// TestTableBoldTextNoTruncation reproduces the "iide" vs "inside" bug.
// When a table cell contains bold text like **bold text inside of tables**,
// the rendered output must contain the full text, not truncated.
func TestTableBoldTextNoTruncation(t *testing.T) {
	text := "| Check | Item |\n|---|---|\n| [x] | **bold text inside of tables** |"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor outside table → rendered mode
	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	// Line 2 is the body row with bold text (line 0=header, line 1=separator)
	bodyLine := snap.Lines[2]
	if len(bodyLine.Spans) == 0 {
		t.Fatal("body line has no spans")
	}

	// Concatenate all span text to check for truncation
	var fullText strings.Builder
	for _, sp := range bodyLine.Spans {
		fullText.WriteString(sp.Text)
	}
	rendered := fullText.String()

	// The rendered text must contain "inside" (not truncated to "iide")
	if !strings.Contains(rendered, "inside") {
		t.Errorf("rendered text missing 'inside': %q", rendered)
	}

	// Also check that bold span text is correct
	foundBold := false
	for _, sp := range bodyLine.Spans {
		if sp.Marks.Has(display.MarkBold) {
			foundBold = true
			if sp.Text != "bold text inside of tables" {
				t.Errorf("bold span text: got %q, want %q", sp.Text, "bold text inside of tables")
			}
		}
	}
	if !foundBold {
		t.Error("expected TokenBold span in table body line")
	}
}

// TestTableNestedInlineSpanBoundaries verifies that the inline emitter correctly
// finds the full extent of emphasis nodes containing nested inline elements.
func TestTableNestedInlineSpanBoundaries(t *testing.T) {
	// Bold with italic inside a table cell
	text := "| **bold *italic* text** | desc |\n|---|---|\n| **bold *italic* text** | desc |"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	// Line 0 is the header with bold+italic
	headerLine := snap.Lines[0]
	if len(headerLine.Spans) == 0 {
		t.Fatal("header line has no spans")
	}

	// Concatenate all span text
	var fullText strings.Builder
	for _, sp := range headerLine.Spans {
		fullText.WriteString(sp.Text)
	}
	rendered := fullText.String()

	// The rendered text should NOT contain raw ** delimiters
	if strings.Contains(rendered, "**") {
		t.Errorf("rendered output contains raw ** delimiters: %q", rendered)
	}

	// Should contain "italic" (the nested inline content)
	if !strings.Contains(rendered, "italic") {
		t.Errorf("rendered output missing 'italic': %q", rendered)
	}
}

// TestTableBoldSpanCellMapConsistency verifies that the CellMap for a bold
// span in a table cell correctly maps each rendered character to a buffer offset.
func TestTableBoldSpanCellMapConsistency(t *testing.T) {
	text := "| **bold** | normal |\n|----------|--------|\n| **bold** | normal |"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	bodyLine := snap.Lines[2]
	for _, sp := range bodyLine.Spans {
		if sp.Marks.Has(display.MarkBold) {
			// CellMap should have same length as Text in runes (visual cells)
			if sp.CellMap == nil {
				t.Error("TokenBold span has nil CellMap")
				continue
			}
			if len(sp.CellMap) != utf8.RuneCountInString(sp.Text) {
				t.Errorf("TokenBold span CellMap length %d != Text rune count %d: text=%q",
					len(sp.CellMap), utf8.RuneCountInString(sp.Text), sp.Text)
			}
			// Each CellMap entry should have a valid buffer offset
			for i, cm := range sp.CellMap {
				if cm.BufOffset < 0 {
					t.Errorf("TokenBold span CellMap[%d] has BufOffset=%d: text=%q",
						i, cm.BufOffset, sp.Text)
				}
			}
		}
	}
}

// TestTableLinkNonFirstCellBufferStart pins BUG6: buildTableStyledSpans
// pinned every emitted span's BufferStart/BufferEnd to the whole table row
// instead of the token's own bounds, so a folded link in a non-first cell
// computed its hidden prefix against everything before it in the row.
func TestTableLinkNonFirstCellBufferStart(t *testing.T) {
	text := "| A | B |\n| --- | --- |\n| x | [bar](baz.md) |"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()
	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	lineStart := buf.LineStart(2)
	lineText := buf.Line(2)
	wantBracket := lineStart + strings.Index(lineText, "[")

	found := false
	for _, sp := range snap.Lines[2].Spans {
		if sp.Kind != display.TokenLink || sp.CellMap == nil {
			continue
		}
		found = true
		if sp.BufferStart != wantBracket {
			t.Errorf("link span BufferStart = %d, want %d (position of '[')", sp.BufferStart, wantBracket)
		}
	}
	if !found {
		t.Fatal("no TokenLink span found on table body line")
	}
}
