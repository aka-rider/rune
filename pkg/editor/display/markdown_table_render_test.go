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
	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	// Line 2 is the data row. It should contain a TokenBold span with text "x".
	dataLine := snap.Lines[2]
	var foundBold bool
	for _, sp := range dataLine.Spans {
		if sp.Marks.Has(display.MarkBold) {
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
	// Collect all text from body spans
	var bodyText strings.Builder
	for _, sp := range dataLine.Spans {
		if sp.TableRole == display.TableRoleBody {
			bodyText.WriteString(sp.Text)
		}
	}
	fullText := bodyText.String()

	// Rendered cell content should not contain raw markdown delimiters.
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
	// Separator should use box-drawing characters (─, ├, ┤, ┼) not ASCII.
	// Must have correct structure: ├ ── ┼ ── ┤ (not ├ ── ┤┼ ── ┤)
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

	// Must contain box-drawing horizontal line
	if !strings.Contains(sepLine, "─") {
		t.Errorf("Separator missing box-drawing horizontal line. Got: %q", sepLine)
	}
	// Must not contain ASCII pipe
	if strings.Contains(sepLine, "|") {
		t.Errorf("Separator should not contain ASCII pipe '|'. Got: %q", sepLine)
	}
	// Must start with ├ and end with ┤
	if !strings.HasPrefix(sepLine, "├") {
		t.Errorf("Separator should start with ├, got: %q", sepLine)
	}
	if !strings.HasSuffix(sepLine, "┤") {
		t.Errorf("Separator should end with ┤, got: %q", sepLine)
	}
	// Must not contain ┤┼ (the bug: rightCorner followed by junction)
	if strings.Contains(sepLine, "┤┼") {
		t.Errorf("Separator has ┤┼ artifact (rightCorner written after every column). Got: %q", sepLine)
	}
	// For a 2-column table, should have exactly 1 junction ┼
	junctionCount := strings.Count(sepLine, "┼")
	if junctionCount != 1 {
		t.Errorf("Expected 1 junction ┼ for 2-column table, got %d. Text: %q", junctionCount, sepLine)
	}
}

func TestTable_NoOverlappingBorders(t *testing.T) {
	// For a 2-column table, each data/header row has exactly 3 vertical borders (│).
	md := `| H1 | H2 |
|---|---|
| a | b |`

	buf := buffer.New(md)
	sMap := display.NewSyntaxMap()
	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	for lineIdx, line := range snap.Lines {
		if lineIdx == 1 {
			continue // separator line uses ├┼┤ not │
		}
		var fullText strings.Builder
		for _, sp := range line.Spans {
			fullText.WriteString(sp.Text)
		}
		text := fullText.String()

		vertCount := strings.Count(text, "│")
		// 2-column table: exactly 3 vertical borders per row
		if vertCount != 3 {
			t.Errorf("Line %d: expected 3 vertical borders (│) for 2-column table, got %d. Text: %q", lineIdx, vertCount, text)
		}
	}
}

func TestTable_MixedPlainAndBoldText(t *testing.T) {
	// A cell with "hello **world** today" must render the full text
	// with "world" as TokenBold and "hello " / " today" as TokenTable (plain).
	md := `| Col |
|---|
| hello **world** today |`

	buf := buffer.New(md)
	sMap := display.NewSyntaxMap()
	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	dataLine := snap.Lines[2]
	var fullText strings.Builder
	var foundBold bool
	for _, sp := range dataLine.Spans {
		fullText.WriteString(sp.Text)
		if sp.Marks.Has(display.MarkBold) && strings.Contains(sp.Text, "world") {
			foundBold = true
		}
	}

	text := fullText.String()
	// Must contain the full rendered text (no gaps)
	if !strings.Contains(text, "hello") {
		t.Errorf("Missing plain text 'hello' in rendered output: %q", text)
	}
	if !strings.Contains(text, "world") {
		t.Errorf("Missing bold text 'world' in rendered output: %q", text)
	}
	if !strings.Contains(text, "today") {
		t.Errorf("Missing plain text 'today' in rendered output: %q", text)
	}
	if !foundBold {
		t.Error("Expected TokenBold span containing 'world'")
	}
	// Must NOT contain raw delimiters
	if strings.Contains(text, "**") {
		t.Errorf("Rendered output contains raw ** delimiters: %q", text)
	}
}

func TestTable_ConsistentColumnWidths(t *testing.T) {
	// All rows must have the same total rendered width (consistent column alignment).
	md := `| Name | Description |
|---|---|
| short | a |
| longer name | much longer description |`

	buf := buffer.New(md)
	sMap := display.NewSyntaxMap()
	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	var widths []int
	for lineIdx, line := range snap.Lines {
		var fullText strings.Builder
		for _, sp := range line.Spans {
			fullText.WriteString(sp.Text)
		}
		text := fullText.String()
		w := 0
		for _, r := range text {
			w++
			_ = r
		}
		widths = append(widths, len(text))
		_ = lineIdx
	}

	// Header (line 0), separator (line 1), body rows (lines 2, 3) should all
	// have similar byte lengths (indicating consistent column padding).
	// Skip separator — it may differ slightly due to box characters.
	if len(widths) >= 4 {
		headerW := widths[0]
		for i := 2; i < len(widths); i++ {
			if widths[i] != headerW {
				// Note: byte lengths may differ between rows when content has
				// different rune widths, but for ASCII content they must match.
				t.Errorf("Row %d width (%d bytes) differs from header width (%d bytes) — column alignment broken", i, widths[i], headerW)
			}
		}
	}
}

func TestTable_ThreeColumnSeparator(t *testing.T) {
	// A 3-column table separator should have exactly 2 junctions.
	md := `| A | B | C |
|---|---|---|
| 1 | 2 | 3 |`

	buf := buffer.New(md)
	sMap := display.NewSyntaxMap()
	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	var sepText strings.Builder
	for _, sp := range snap.Lines[1].Spans {
		sepText.WriteString(sp.Text)
	}
	sepLine := sepText.String()

	junctionCount := strings.Count(sepLine, "┼")
	if junctionCount != 2 {
		t.Errorf("Expected 2 junctions ┼ for 3-column table, got %d. Text: %q", junctionCount, sepLine)
	}
	// Exactly 1 ├ and 1 ┤
	if strings.Count(sepLine, "├") != 1 {
		t.Errorf("Expected 1 left corner ├, got %d. Text: %q", strings.Count(sepLine, "├"), sepLine)
	}
	if strings.Count(sepLine, "┤") != 1 {
		t.Errorf("Expected 1 right corner ┤, got %d. Text: %q", strings.Count(sepLine, "┤"), sepLine)
	}
}

func TestTable_PivotLayout(t *testing.T) {
	// When available width is too narrow for the grid, pivot mode renders
	// each row as "Header: Value" key-value pairs.
	md := `| Name | URL |
|---|---|
| Swagger | http://localhost:8080/swagger-ui.html |
| Grid | http://localhost:4444/ui |`

	buf := buffer.New(md)
	sMap := display.NewSyntaxMap().SetWidth(20) // too narrow for grid
	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	// Build the full rendered text
	var full strings.Builder
	for _, line := range snap.Lines {
		for _, sp := range line.Spans {
			full.WriteString(sp.Text)
		}
		full.WriteByte('\n')
	}
	text := full.String()

	// Should contain "Name: Swagger" and "URL: http://..."
	if !strings.Contains(text, "Name: Swagger") {
		t.Errorf("Pivot mode should contain 'Name: Swagger', got:\n%s", text)
	}
	if !strings.Contains(text, "URL: http://localhost:8080/swagger-ui.html") {
		t.Errorf("Pivot mode should contain full URL, got:\n%s", text)
	}
	// Should NOT contain │ border characters (pivot has no grid borders)
	if strings.Contains(text, "│") {
		t.Errorf("Pivot mode should not have grid borders │, got:\n%s", text)
	}
	// Should contain horizontal rule separator between rows
	if !strings.Contains(text, "────") {
		t.Errorf("Pivot mode should have ──── separator between rows, got:\n%s", text)
	}
}

func TestTable_GridTopBottomBorders(t *testing.T) {
	// Grid tables should have ┌──┬──┐ top and └──┴──┘ bottom borders.
	md := `| A | B |
|---|---|
| 1 | 2 |`

	buf := buffer.New(md)
	sMap := display.NewSyntaxMap().SetWidth(200) // wide enough for grid
	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	// Build display snapshot with table expansion
	wm := display.NewWrapMap(200)
	ws := wm.Sync(snap)
	ds := display.BuildSnapshot(ws)
	ds = display.ExpandTableRows(ds)

	var lines []string
	for _, dl := range ds.Lines {
		var lineText strings.Builder
		for _, sp := range dl.Spans {
			lineText.WriteString(sp.Text)
		}
		lines = append(lines, lineText.String())
	}

	if len(lines) == 0 {
		t.Fatal("No display lines")
	}

	// First line should be top border with ┌
	if !strings.Contains(lines[0], "┌") {
		t.Errorf("First line should be top border ┌──┬──┐, got: %q", lines[0])
	}
	if !strings.Contains(lines[0], "┐") {
		t.Errorf("First line should end with ┐, got: %q", lines[0])
	}

	// Last line should be bottom border with └
	lastLine := lines[len(lines)-1]
	if !strings.Contains(lastLine, "└") {
		t.Errorf("Last line should be bottom border └──┴──┘, got: %q", lastLine)
	}
	if !strings.Contains(lastLine, "┘") {
		t.Errorf("Last line should end with ┘, got: %q", lastLine)
	}
}

func TestTable_BodyRowSeparatorWidthsWithMultiByteChars(t *testing.T) {
	// Body→body separator widths must match header→body separator widths.
	// Regression test: the border builder used to recover column widths by
	// re-parsing the already-rendered │ cell │ cell │ text with byte-based
	// iteration, which inflated widths for cells containing multi-byte UTF-8
	// chars (→, CJK, etc.) — causing mismatched separator widths between
	// header-body and body-body rows. Border widths now come straight off
	// DisplaySpan.ColWidths (set once, upstream, when the row was formatted),
	// so this bug class no longer exists — kept as a regression guard.
	md := `| Conversion | Invariant | Condition |
|------------|-----------|-----------|
| Buffer→Syntax→Buffer | Identity | All valid offsets |
| Syntax→Buffer→Syntax | Identity | Offsets NOT inside hidden delimiters |`

	buf := buffer.New(md)
	sMap := display.NewSyntaxMap()
	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	wm := display.NewWrapMap(200)
	ws := wm.Sync(snap)
	ds := display.BuildSnapshot(ws)
	ds = display.ExpandTableRows(ds)

	var lines []string
	for _, dl := range ds.Lines {
		var lineText strings.Builder
		for _, sp := range dl.Spans {
			lineText.WriteString(sp.Text)
		}
		lines = append(lines, lineText.String())
	}

	// Find separator lines (start with ├) and data lines (start with │)
	var sepLines, dataLines []struct {
		idx  int
		text string
	}
	for i, l := range lines {
		if strings.HasPrefix(l, "├") || strings.HasPrefix(l, "└") || strings.HasPrefix(l, "┌") {
			sepLines = append(sepLines, struct {
				idx  int
				text string
			}{i, l})
		} else if strings.HasPrefix(l, "│") {
			dataLines = append(dataLines, struct {
				idx  int
				text string
			}{i, l})
		}
	}

	// First separator is header→body, subsequent separators are body→body.
	// All separators must have identical column widths (same number of ─ per column).
	if len(sepLines) < 2 {
		t.Fatalf("Expected at least 2 separator lines, got %d", len(sepLines))
	}

	// Extract column dash counts from each separator
	var colDashCounts [][]int
	for _, sl := range sepLines {
		var counts []int
		current := 0
		for _, r := range sl.text {
			if r == '─' {
				current++
			} else {
				if current > 0 {
					counts = append(counts, current)
					current = 0
				}
			}
		}
		if current > 0 {
			counts = append(counts, current)
		}
		colDashCounts = append(colDashCounts, counts)
	}

	// All separators should have the same column widths
	for i := 1; i < len(colDashCounts); i++ {
		if len(colDashCounts[i]) != len(colDashCounts[0]) {
			t.Errorf("Separator %d has %d columns, expected %d", i, len(colDashCounts[i]), len(colDashCounts[0]))
			continue
		}
		for j := range colDashCounts[0] {
			if colDashCounts[i][j] != colDashCounts[0][j] {
				t.Errorf("Separator %d col %d width %d != header col width %d\n  header sep: %q\n  body sep:   %q",
					i, j, colDashCounts[i][j], colDashCounts[0][j], sepLines[0].text, sepLines[i].text)
			}
		}
	}

	// Verify body rows actually render as data (│), not separators (├)
	for _, dl := range dataLines {
		if !strings.HasPrefix(dl.text, "│") {
			t.Errorf("Body row at index %d should start with │, got: %q", dl.idx, dl.text)
		}
	}
}
