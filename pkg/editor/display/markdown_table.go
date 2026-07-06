package display

import (
	"fmt"
	"strings"
)

// renderedCellData holds the rendered (delimiter-stripped) text and cell mapping
// for a single table cell, extracted from inline spans or raw source.
type renderedCellData struct {
	text  string        // rendered text (without markdown delimiters)
	cm    []CellMapping // per-byte buffer offsets for rendered text
	width int           // visual width of rendered text
}

// tableRenderedSpans produces spans for a table line in rendered mode.
// It formats cells with padding and alignment, and identifies line roles.
// When parsed spans are available, it uses rendered text (delimiter-stripped)
// for correct column widths and styled spans.
// availableWidth determines the layout state (grid/wrapped/pivoted).
func tableRenderedSpans(block mdBlock, lineIdx int, lineText string, lineStart int, mdSpans []mdSpan, parsed []parsedLine, availableWidth int) []SyntaxSpan {
	role := tableLineRole(block, lineIdx)

	// If no column widths computed, fall back to raw text
	if len(block.colWidths) == 0 {
		return []SyntaxSpan{{
			Text:        lineText,
			Kind:        TokenTable,
			State:       Rendered,
			BufferStart: lineStart,
			BufferEnd:   lineStart + len(lineText),
			BlockID:     block.id,
			BlockStart:  block.startOff,
			BlockEnd:    block.endOff,
			TableRole:   role,
		}}
	}

	// Choose layout based on available width
	layout := TableLayoutGrid
	if availableWidth > 0 {
		layout = chooseTableLayout(block.colWidths, block.minColWidths, availableWidth)
	}

	if layout == TableLayoutPivoted {
		return formatPivotedRow(block, lineIdx, lineText, lineStart, parsed, role, availableWidth)
	}

	// Grid or Wrapped layout — both render as a grid, but Wrapped uses
	// proportionally constrained column widths with word-wrap inside cells.
	colWidths := block.colWidths
	if layout == TableLayoutWrapped {
		colWidths = constrainColWidths(colWidths, block.minColWidths, availableWidth)
	}

	// For separator lines, render a formatted separator using box-drawing characters
	if role == TableRoleSeparator {
		spans := formatTableSeparatorSpansWithWidths(colWidths, lineStart, lineText, block)
		for i := range spans {
			spans[i].TableLayout = layout
		}
		return spans
	}

	// For header and body lines, parse and pad cells
	var lineSpans []mdSpan
	if parsed != nil && lineIdx < len(parsed) {
		lineSpans = parsed[lineIdx].spans
	}

	// Extract rendered cell data from spans
	renderedCells := extractRenderedCellData(lineText, lineStart, lineSpans, len(block.colWidths))

	if layout == TableLayoutWrapped {
		return formatWrappedRow(block, lineIdx, renderedCells, colWidths, block.alignments, lineStart, lineText, lineSpans, role)
	}

	// Build grid row spans
	return formatGridRow(block, lineIdx, renderedCells, colWidths, block.alignments, lineStart, lineText, lineSpans, role, layout)
}

// formatGridRow formats a table row as a standard single-line grid with styled spans.
func formatGridRow(block mdBlock, lineIdx int, renderedCells []renderedCellData, colWidths []int, alignments []int, lineStart int, lineText string, lineSpans []mdSpan, role TableRoleKind, layout TableLayoutKind) []SyntaxSpan {
	formatted, cm := formatTableRowRendered(renderedCells, colWidths, alignments, lineStart, lineText)

	if len(lineSpans) == 0 {
		return []SyntaxSpan{{
			Text:        formatted,
			Kind:        TokenTable,
			State:       Rendered,
			BufferStart: lineStart,
			BufferEnd:   lineStart + len(lineText),
			CellMap:     cm,
			BlockID:     block.id,
			BlockStart:  block.startOff,
			BlockEnd:    block.endOff,
			TableRole:   role,
			TableLayout: layout,
			ColWidths:   colWidths,
		}}
	}

	spans := buildTableStyledSpans(block, lineIdx, formatted, cm, lineStart, lineText, lineSpans, role)
	// Set layout and column widths on all spans — ExpandTableRows' border
	// builder reads ColWidths back off whichever span it finds on the line
	// (buildTableBorder/table_rows.go), so every span this row produces must
	// carry the same value.
	for i := range spans {
		spans[i].TableLayout = layout
		spans[i].ColWidths = colWidths
	}
	return spans
}

// formatWrappedRow formats a table row as a multi-line grid with word-wrapped cells.
// Each cell's content is wrapped at word boundaries to fit within its constrained
// column width. The row may span multiple visual lines. Lines are joined with \n
// for ExpandTableRows to split into separate display rows.
func formatWrappedRow(block mdBlock, lineIdx int, renderedCells []renderedCellData, colWidths []int, alignments []int, lineStart int, lineText string, lineSpans []mdSpan, role TableRoleKind) []SyntaxSpan {
	numCols := len(colWidths)

	// Word-wrap each cell's content to its column width
	wrappedCells := make([][]string, numCols)
	maxLines := 1
	for col := 0; col < numCols; col++ {
		rc := renderedCellData{text: "", width: 0}
		if col < len(renderedCells) {
			rc = renderedCells[col]
		}
		w := colWidths[col]
		wrappedCells[col] = wrapCellText(rc.text, w)
		if len(wrappedCells[col]) > maxLines {
			maxLines = len(wrappedCells[col])
		}
	}

	// Build multi-line output: each visual line has │ cell │ cell │ format
	var b strings.Builder
	var cm []CellMapping

	for row := 0; row < maxLines; row++ {
		if row > 0 {
			b.WriteByte('\n')
			cm = append(cm, CellMapping{BufOffset: -1})
		}

		for col, w := range colWidths {
			// Opening border — CellMap is per-rune (§1.5 display side; D1:
			// len(CellMap) == RuneCount(Text)): '│' is 3 bytes but ONE visual
			// cell, so exactly one mapping.
			if col == 0 {
				b.WriteRune('│')
				cm = append(cm, CellMapping{BufOffset: -1})
			}
			// Left padding
			b.WriteByte(' ')
			cm = append(cm, CellMapping{BufOffset: -1})

			// Cell content for this visual row
			cellText := ""
			if row < len(wrappedCells[col]) {
				cellText = wrappedCells[col][row]
			}
			cellWidth := runewidthSafe(cellText)
			pad := w - cellWidth
			if pad < 0 {
				pad = 0
			}

			// Write cell text (left-aligned for wrapped mode) — one mapping
			// per RUNE (range over a string yields runes), not per byte.
			b.WriteString(cellText)
			for range cellText {
				cm = append(cm, CellMapping{BufOffset: -1})
			}
			// Right padding
			for i := 0; i < pad; i++ {
				b.WriteByte(' ')
				cm = append(cm, CellMapping{BufOffset: -1})
			}

			// Right padding space + closing border (one mapping per rune —
			// see the opening-border comment above)
			b.WriteByte(' ')
			cm = append(cm, CellMapping{BufOffset: -1})
			b.WriteRune('│')
			cm = append(cm, CellMapping{BufOffset: -1})
		}
	}

	return []SyntaxSpan{{
		Text:        b.String(),
		Kind:        TokenTable,
		State:       Rendered,
		BufferStart: lineStart,
		BufferEnd:   lineStart + len(lineText),
		CellMap:     cm,
		BlockID:     block.id,
		BlockStart:  block.startOff,
		BlockEnd:    block.endOff,
		TableRole:   role,
		TableLayout: TableLayoutWrapped,
		ColWidths:   colWidths,
	}}
}

// formatPivotedRow renders a table row in pivoted (key-value) mode.
// Header and separator rows are suppressed (return empty span).
// Body rows are rendered as "Header: Value" per column, joined by newlines.
// A horizontal rule separator is placed between rows.
func formatPivotedRow(block mdBlock, lineIdx int, lineText string, lineStart int, parsed []parsedLine, role TableRoleKind, availableWidth int) []SyntaxSpan {
	numCols := len(block.colWidths)

	// Suppress header and separator rows in pivoted mode
	if role == TableRoleHeader || role == TableRoleSeparator {
		return []SyntaxSpan{{
			Text:        "",
			Kind:        TokenTable,
			State:       Rendered,
			BufferStart: lineStart,
			BufferEnd:   lineStart + len(lineText),
			BlockID:     block.id,
			BlockStart:  block.startOff,
			BlockEnd:    block.endOff,
			TableRole:   role,
			TableLayout: TableLayoutPivoted,
		}}
	}

	// Extract rendered cell data for this row
	var lineSpans []mdSpan
	if parsed != nil && lineIdx < len(parsed) {
		lineSpans = parsed[lineIdx].spans
	}
	renderedCells := extractRenderedCellData(lineText, lineStart, lineSpans, numCols)

	// Build "Header: Value" pairs separated by newlines
	var b strings.Builder
	var cm []CellMapping

	// Add a horizontal separator before the first body row
	// and between subsequent body rows
	isFirstBody := (lineIdx == block.sepLine+1) || (block.sepLine < 0 && lineIdx == block.headerEnd+1)
	if !isFirstBody {
		// Separator between rows: ────────
		sepWidth := availableWidth
		if sepWidth <= 0 {
			sepWidth = 40
		}
		// One mapping per RUNE ('─' is 3 bytes but one visual cell — §1.5
		// display side; D1: len(CellMap) == RuneCount(Text)).
		for i := 0; i < sepWidth; i++ {
			b.WriteRune('─')
			cm = append(cm, CellMapping{BufOffset: -1})
		}
		b.WriteByte('\n')
		cm = append(cm, CellMapping{BufOffset: -1})
	}

	for col := 0; col < numCols; col++ {
		rc := renderedCellData{text: "", width: 0}
		if col < len(renderedCells) {
			rc = renderedCells[col]
		}

		// Get header label
		label := ""
		if col < len(block.headerCells) {
			label = block.headerCells[col]
		}
		if label == "" {
			label = fmt.Sprintf("Col%d", col+1)
		}

		// Format: "  Label: Value"
		b.WriteString("  ")
		cm = append(cm, CellMapping{BufOffset: -1}, CellMapping{BufOffset: -1})

		b.WriteString(label)
		for range label { // per RUNE, not per byte
			cm = append(cm, CellMapping{BufOffset: -1})
		}

		b.WriteString(": ")
		cm = append(cm, CellMapping{BufOffset: -1}, CellMapping{BufOffset: -1})

		// Write cell value with proper CellMapping
		b.WriteString(rc.text)
		if len(rc.cm) > 0 {
			cm = append(cm, rc.cm...)
		} else {
			for range rc.text { // per RUNE, not per byte
				cm = append(cm, CellMapping{BufOffset: -1})
			}
		}

		// Newline between columns (except last)
		if col < numCols-1 {
			b.WriteByte('\n')
			cm = append(cm, CellMapping{BufOffset: -1})
		}
	}

	return []SyntaxSpan{{
		Text:        b.String(),
		Kind:        TokenTable,
		State:       Rendered,
		BufferStart: lineStart,
		BufferEnd:   lineStart + len(lineText),
		CellMap:     cm,
		BlockID:     block.id,
		BlockStart:  block.startOff,
		BlockEnd:    block.endOff,
		TableRole:   role,
		TableLayout: TableLayoutPivoted,
	}}
}
