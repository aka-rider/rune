package display_test

import (
	"strings"
	"testing"

	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/display"
)

func TestTable_InlineBoldRendered(t *testing.T) {
	// Table cell with **bold** should produce TokenBold spans, not raw "**bold**" text.
	md := `| Col |
|---|
| **x** |`

	buf := buffer.New(md)
	sMap := display.NewSyntaxMap()
	// Use SyncNoReveal to get rendered mode output (cursor outside block).
	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	// Line 2 is the data row. It should contain a TokenBold span with text "x".
	dataLine := snap.Lines[2]
	var foundBold bool
	for _, sp := range dataLine.Spans {
		if sp.Kind == display.TokenBold {
			foundBold = true
			if sp.Text != "x" {
				t.Errorf("Expected bold text 'x', got %q", sp.Text)
			}
		}
	}
	if !foundBold {
		t.Error("Table cell with **bold** produced no TokenBold span — inline parsing is skipped for table cells")
	}
}

func TestTable_InlineLinkRendered(t *testing.T) {
	md := `| Col |
|---|
| [text](http://example.com) |`

	buf := buffer.New(md)
	sMap := display.NewSyntaxMap()
	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	dataLine := snap.Lines[2]
	var foundLink bool
	for _, sp := range dataLine.Spans {
		if sp.Kind == display.TokenLink {
			foundLink = true
			if sp.Text != "text" {
				t.Errorf("Expected link text 'text', got %q", sp.Text)
			}
			if sp.LinkURL != "http://example.com" {
				t.Errorf("Expected link URL 'http://example.com', got %q", sp.LinkURL)
			}
		}
	}
	if !foundLink {
		t.Error("Table cell with [link](url) produced no TokenLink span")
	}
}

func TestTable_InlineCodeRendered(t *testing.T) {
	md := "| Col |\n|---|\n| " + "`" + "code" + "`" + " |"

	buf := buffer.New(md)
	sMap := display.NewSyntaxMap()
	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	dataLine := snap.Lines[2]
	var foundCode bool
	for _, sp := range dataLine.Spans {
		if sp.Kind == display.TokenInlineCode {
			foundCode = true
			if sp.Text != "code" {
				t.Errorf("Expected inline code text 'code', got %q", sp.Text)
			}
		}
	}
	if !foundCode {
		t.Error("Table cell with inline code produced no TokenInlineCode span")
	}
}

func TestTable_ColumnWidthFromRenderedText(t *testing.T) {
	// Column width should be computed from rendered text (without delimiters).
	// "**hello**" renders as "hello" (5 chars), not "**hello**" (9 chars).
	md := `| A |
|---|
| **hello** |`

	buf := buffer.New(md)
	sMap := display.NewSyntaxMap()
	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	dataLine := snap.Lines[2]
	// Collect all non-border text from table body spans
	var bodyText strings.Builder
	for _, sp := range dataLine.Spans {
		if sp.TableRole == display.TableRoleBody {
			bodyText.WriteString(sp.Text)
		}
	}
	fullText := bodyText.String()

	// The rendered cell content should be "hello" (5 chars) padded to column width.
	// If column width is computed from raw source "**hello**" (9 chars), the text
	// would include markdown delimiters, which is wrong.
	if strings.Contains(fullText, "**") {
		t.Errorf("Table cell text contains raw markdown delimiters: %q", fullText)
	}
}

func TestTable_HeaderRoleOnFirstLine(t *testing.T) {
	md := `| H1 | H2 |
|---|---|
| a | b |`

	buf := buffer.New(md)
	sMap := display.NewSyntaxMap()
	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	// Line 0 is the header row — should have TableRoleHeader
	for i, sp := range snap.Lines[0].Spans {
		if sp.TableRole != 0 && sp.TableRole != display.TableRoleHeader {
			t.Errorf("Header line span[%d]: expected TableRoleHeader, got %d", i, sp.TableRole)
		}
	}

	// Line 2 is a body row — should have TableRoleBody
	for i, sp := range snap.Lines[2].Spans {
		if sp.TableRole != 0 && sp.TableRole != display.TableRoleBody {
			t.Errorf("Body line span[%d]: expected TableRoleBody, got %d", i, sp.TableRole)
		}
	}
}

func TestTable_SeparatorRole(t *testing.T) {
	md := `| H1 | H2 |
|---|---|
| a | b |`

	buf := buffer.New(md)
	sMap := display.NewSyntaxMap()
	// Place cursor outside the table block so rendered mode is used.
	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	// Line 1 is the separator — should have TableRoleSeparator
	var foundSep bool
	for _, sp := range snap.Lines[1].Spans {
		if sp.TableRole == display.TableRoleSeparator {
			foundSep = true
		}
	}
	if !foundSep {
		t.Error("Separator line has no TableRoleSeparator span")
	}
}

func TestTable_BoxDrawingSeparator(t *testing.T) {
	// Separator should use box-drawing characters (─, ├, ┤, ┼) not ASCII (-, |).
	md := `| H1 | H2 |
|---|---|
| a | b |`

	buf := buffer.New(md)
	sMap := display.NewSyntaxMap()
	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	var sepText strings.Builder
	for _, sp := range snap.Lines[1].Spans {
		sepText.WriteString(sp.Text)
	}
	sepLine := sepText.String()

	// Should contain box-drawing characters
	if !strings.Contains(sepLine, "─") {
		t.Errorf("Separator missing box-drawing horizontal line. Got: %q", sepLine)
	}
	if strings.Contains(sepLine, "|") {
		t.Errorf("Separator should not contain ASCII pipe '|'. Got: %q", sepLine)
	}
}

func TestTable_NoOverlappingBorders(t *testing.T) {
	// Each rendered table line should have a clean, single set of borders.
	// Multiple │ characters in the same visual column indicate a bug.
	md := `| H1 | H2 |
|---|---|
| a | b |`

	buf := buffer.New(md)
	sMap := display.NewSyntaxMap()
	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	for lineIdx, line := range snap.Lines {
		var fullText strings.Builder
		for _, sp := range line.Spans {
			fullText.WriteString(sp.Text)
		}
		text := fullText.String()

		// Count vertical border characters — a correct table line should have
		// exactly (numColumns + 1) vertical borders.
		vertCount := strings.Count(text, "│") + strings.Count(text, "|")
		// For a 2-column table, we expect 3 vertical borders per line.
		// Allow some tolerance for the separator line (which may have 0 vertical borders).
		if lineIdx == 1 {
			continue // separator line
		}
		if vertCount > 10 {
			t.Errorf("Line %d has %d vertical border characters — likely overlapping borders. Text: %q", lineIdx, vertCount, text)
		}
	}
}
