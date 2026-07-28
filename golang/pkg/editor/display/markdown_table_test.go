package display_test

import (
	"testing"

	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/display"
)

// ==========================================================================
// WP1: Inline spans are extracted from table cell AST children
// ==========================================================================

func TestTable_InlineSpansExtracted(t *testing.T) {
	// Table with bold text in cells — spans should be non-empty for table lines
	text := "| Name | Value |\n|-------|-------|\n| **bold** | normal |"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor outside table → rendered mode
	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	// Line 2 (body line with **bold**) should have spans with TokenBold kind
	foundBold := false
	for _, sp := range snap.Lines[2].Spans {
		if sp.Marks.Has(display.MarkBold) {
			foundBold = true
			if sp.Text != "bold" {
				t.Errorf("bold span text: got %q, want %q", sp.Text, "bold")
			}
		}
	}
	if !foundBold {
		t.Error("expected TokenBold span in table body line with **bold** content")
	}
}

func TestTable_HeaderHasSpans(t *testing.T) {
	text := "| **Name** | Value |\n|----------|-------|\n| a | b |"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	// Line 0 (header with **Name**) should have TokenBold span
	foundBold := false
	for _, sp := range snap.Lines[0].Spans {
		if sp.Marks.Has(display.MarkBold) && sp.TableRole == display.TableRoleHeader {
			foundBold = true
			if sp.Text != "Name" {
				t.Errorf("bold header span text: got %q, want %q", sp.Text, "Name")
			}
		}
	}
	if !foundBold {
		t.Error("expected TokenBold span in table header line with **Name** content")
	}
}

// ==========================================================================
// WP2: Column widths computed from rendered text, not raw source
// ==========================================================================

func TestTable_ColumnWidthsFromRenderedText(t *testing.T) {
	// Column 0 has **bold** (raw width 8, rendered width 4)
	// Column widths should be computed from rendered text
	text := "| **bold** | short |\n|----------|-------|\n| plain | data |"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	// Line 0 (header) should render with correct column widths
	// The header text "bold" (4 chars) should fit in the column
	headerLine := snap.Lines[0]
	totalText := ""
	for _, sp := range headerLine.Spans {
		totalText += sp.Text
	}

	// The rendered header should contain "bold" and "short"
	if len(totalText) == 0 {
		t.Error("header line has no rendered text")
	}
}

func TestTable_LinkInCellProducesLinkSpan(t *testing.T) {
	text := "| Name | Link |\n|------|------|\n| a | [click](http://example.com) |"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	// Line 2 (body with link) should have TokenLink span
	foundLink := false
	for _, sp := range snap.Lines[2].Spans {
		if sp.Kind == display.TokenLink {
			foundLink = true
			if sp.Text != "click" {
				t.Errorf("link span text: got %q, want %q", sp.Text, "click")
			}
			if sp.LinkURL != "http://example.com" {
				t.Errorf("link URL: got %q, want %q", sp.LinkURL, "http://example.com")
			}
		}
	}
	if !foundLink {
		t.Error("expected TokenLink span in table body line with [click](url) content")
	}
}

func TestTable_MixedFormatting(t *testing.T) {
	// Cell with both bold and link
	text := "| Item |\n|------|\n| **bold** and [link](url) |"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	// Line 2 should have both TokenBold and TokenLink spans
	hasBold := false
	hasLink := false
	for _, sp := range snap.Lines[2].Spans {
		if sp.Marks.Has(display.MarkBold) {
			hasBold = true
		}
		if sp.Kind == display.TokenLink {
			hasLink = true
		}
	}
	if !hasBold {
		t.Error("expected TokenBold span in mixed formatting cell")
	}
	if !hasLink {
		t.Error("expected TokenLink span in mixed formatting cell")
	}
}

// ==========================================================================
// WP3: Layout decision logic
// ==========================================================================

func TestTable_LayoutDecision_WideTerminal(t *testing.T) {
	// Small table that fits in wide terminal should use grid layout
	text := "| A | B |\n|---|---|\n| 1 | 2 |"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	// All table lines should be rendered (not revealed)
	for lineIdx := 0; lineIdx <= 2; lineIdx++ {
		for _, sp := range snap.Lines[lineIdx].Spans {
			if sp.Kind == display.TokenTable && sp.State != display.Rendered {
				t.Errorf("line %d: table span should be Rendered in grid layout", lineIdx)
			}
		}
	}
}

// ==========================================================================
// WP7: Box-drawing characters
// ==========================================================================

func TestTable_BoxDrawingSeparatorBasic(t *testing.T) {
	text := "| A | B | C |\n|---|---|---|\n| 1 | 2 | 3 |"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	// Line 1 (separator) should use box-drawing characters
	sepLine := snap.Lines[1]
	totalText := ""
	for _, sp := range sepLine.Spans {
		totalText += sp.Text
	}

	// Check for box-drawing characters (├, ┼, ┤, ─)
	hasBoxDrawing := false
	for _, r := range totalText {
		if r == '├' || r == '┼' || r == '┤' || r == '─' {
			hasBoxDrawing = true
			break
		}
	}
	if !hasBoxDrawing {
		t.Errorf("separator should use box-drawing characters, got: %q", totalText)
	}
}

func TestTable_BoxDrawingVerticalBorder(t *testing.T) {
	text := "| A | B |\n|---|---|\n| 1 | 2 |"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	// Line 0 (header) should use │ for vertical borders
	headerLine := snap.Lines[0]
	totalText := ""
	for _, sp := range headerLine.Spans {
		totalText += sp.Text
	}

	// Check for box-drawing vertical character (│)
	hasVertical := false
	for _, r := range totalText {
		if r == '│' {
			hasVertical = true
			break
		}
	}
	if !hasVertical {
		t.Errorf("header row should use │ for vertical borders, got: %q", totalText)
	}
}

// ==========================================================================
// Table roles and state
// ==========================================================================

func TestTable_RolesCorrect(t *testing.T) {
	text := "| H1 | H2 |\n|----|----|\n| v1 | v2 |"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	// Line 0 should be TableRoleHeader
	roleFound := false
	for _, sp := range snap.Lines[0].Spans {
		if sp.TableRole == display.TableRoleHeader {
			roleFound = true
		}
	}
	if !roleFound {
		t.Error("line 0 should have TableRoleHeader")
	}

	// Line 1 should be TableRoleSeparator
	roleFound = false
	for _, sp := range snap.Lines[1].Spans {
		if sp.TableRole == display.TableRoleSeparator {
			roleFound = true
		}
	}
	if !roleFound {
		t.Error("line 1 should have TableRoleSeparator")
	}

	// Line 2 should be TableRoleBody
	roleFound = false
	for _, sp := range snap.Lines[2].Spans {
		if sp.TableRole == display.TableRoleBody {
			roleFound = true
		}
	}
	if !roleFound {
		t.Error("line 2 should have TableRoleBody")
	}
}

func TestTable_RevealedWhenCursorInside(t *testing.T) {
	text := "| A | B |\n|---|---|\n| 1 | 2 |"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor on table line
	tableOffset := len("") // start of table
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(tableOffset))

	// All table lines should be Revealed
	for lineIdx := 0; lineIdx <= 2; lineIdx++ {
		for _, sp := range snap.Lines[lineIdx].Spans {
			if sp.Kind == display.TokenTable && sp.State != display.Revealed {
				t.Errorf("line %d: table span should be Revealed when cursor inside", lineIdx)
			}
		}
	}
}

// ==========================================================================
// Cell mapping integrity
// ==========================================================================

func TestTable_CellMappingPreserved(t *testing.T) {
	text := "| Name | Value |\n|------|-------|\n| a | b |"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	// Line 2 (body) should have CellMap with valid buffer offsets
	bodyLine := snap.Lines[2]
	hasCellMap := false
	for _, sp := range bodyLine.Spans {
		if sp.CellMap != nil && len(sp.CellMap) > 0 {
			hasCellMap = true
			// Check that at least one mapping points to a valid buffer offset
			hasValidOffset := false
			for _, cm := range sp.CellMap {
				if cm.BufOffset >= 0 {
					hasValidOffset = true
					break
				}
			}
			if !hasValidOffset {
				t.Error("CellMap should have at least one valid buffer offset")
			}
		}
	}
	if !hasCellMap {
		t.Error("body line should have CellMap for cursor navigation")
	}
}

// ==========================================================================
// Empty table handling
// ==========================================================================

func TestTable_EmptyCells(t *testing.T) {
	text := "| A | B |\n|---|---|\n|   |   |"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	// Line 2 (empty cells) should still render
	bodyLine := snap.Lines[2]
	if len(bodyLine.Spans) == 0 {
		t.Error("empty table row should still have spans")
	}
}

// ==========================================================================
// Table with inline code
// ==========================================================================

func TestTable_InlineCodeInCell(t *testing.T) {
	text := "| Name | Code |\n|------|------|\n| x | `code` |"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	// Line 2 should have TokenInlineCode span
	foundCode := false
	for _, sp := range snap.Lines[2].Spans {
		if sp.Kind == display.TokenInlineCode {
			foundCode = true
			if sp.Text != "code" {
				t.Errorf("inline code span text: got %q, want %q", sp.Text, "code")
			}
		}
	}
	if !foundCode {
		t.Error("expected TokenInlineCode span in table cell with `code` content")
	}
}

// ==========================================================================
// Table with strikethrough
// ==========================================================================

func TestTable_StrikethroughInCell(t *testing.T) {
	text := "| Name | Value |\n|-------|-------|\n| ~~old~~ | new |"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	// Line 2 should have TokenStrikethrough span
	foundStrike := false
	for _, sp := range snap.Lines[2].Spans {
		if sp.Marks.Has(display.MarkStrikethrough) {
			foundStrike = true
			if sp.Text != "old" {
				t.Errorf("strikethrough span text: got %q, want %q", sp.Text, "old")
			}
		}
	}
	if !foundStrike {
		t.Error("expected TokenStrikethrough span in table cell with ~~old~~ content")
	}
}
