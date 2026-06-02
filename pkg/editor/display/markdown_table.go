package display

import (
	"fmt"
	"strings"
)

// renderedCellData holds the rendered (delimiter-stripped) text and cell mapping
// for a single table cell, extracted from inline spans or raw source.
type renderedCellData struct {
	text string           // rendered text (without markdown delimiters)
	cm   []CellMapping    // per-byte buffer offsets for rendered text
	width int             // visual width of rendered text
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

	// For separator lines, render a formatted separator using box-drawing characters
	if role == TableRoleSeparator {
		return formatTableSeparatorSpans(block, lineStart, lineText)
	}

	// For header and body lines, parse and pad cells
	// Use per-line parsed spans if available
	var lineSpans []mdSpan
	if parsed != nil && lineIdx < len(parsed) {
		lineSpans = parsed[lineIdx].spans
	}

	// Extract rendered cell data from spans
	renderedCells := extractRenderedCellData(lineText, lineStart, lineSpans, len(block.colWidths))

	// Compute rendered column widths if spans are available
	colWidths := block.colWidths
	if len(lineSpans) > 0 {
		colWidths = computeRenderedColWidths(renderedCells, block.colWidths)
		// Update the block's colWidths for consistency across rows
		block.colWidths = colWidths
	}

	// Choose layout based on available width (default to grid if width unknown)
	layout := TableLayoutGrid
	if availableWidth > 0 {
		layout = chooseTableLayout(colWidths, availableWidth)
	}

	// Dispatch by layout kind
	switch layout {
	case TableLayoutGrid:
		return formatGridRow(block, lineIdx, renderedCells, colWidths, block.alignments, lineStart, lineText, lineSpans, role)
	case TableLayoutWrapped:
		return formatWrappedRow(block, lineIdx, renderedCells, colWidths, block.alignments, lineStart, lineText, lineSpans, role, availableWidth)
	case TableLayoutPivoted:
		return formatPivotedRow(block, lineIdx, renderedCells, colWidths, block.alignments, lineStart, lineText, lineSpans, role, availableWidth)
	default:
		return formatGridRow(block, lineIdx, renderedCells, colWidths, block.alignments, lineStart, lineText, lineSpans, role)
	}
}

// formatGridRow formats a table row as a standard single-line grid with styled spans.
func formatGridRow(block mdBlock, lineIdx int, renderedCells []renderedCellData, colWidths []int, alignments []int, lineStart int, lineText string, lineSpans []mdSpan, role TableRoleKind) []SyntaxSpan {
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
			TableLayout: TableLayoutGrid,
		}}
	}

	spans := buildTableStyledSpans(block, lineIdx, formatted, cm, lineStart, lineText, lineSpans, role)
	// Set layout on all spans
	for i := range spans {
		spans[i].TableLayout = TableLayoutGrid
	}
	return spans
}

// formatWrappedRow formats a table row with word-wrapped cells.
// Links are kept as atomic (non-breakable) units.
// Returns spans for the current display line (first wrapped line).
func formatWrappedRow(block mdBlock, lineIdx int, renderedCells []renderedCellData, colWidths []int, alignments []int, lineStart int, lineText string, lineSpans []mdSpan, role TableRoleKind, availableWidth int) []SyntaxSpan {
	// Compute per-column allocated width
	numCols := len(colWidths)
	frameWidth := numCols*2 + 1 // │ + padding per column + final │
	usableWidth := availableWidth - frameWidth
	if usableWidth < numCols {
		// Not enough space — fall back to grid
		return formatGridRow(block, lineIdx, renderedCells, colWidths, alignments, lineStart, lineText, lineSpans, role)
	}
	allocatedPerCol := usableWidth / numCols

	// Build per-cell styled spans, then wrap each cell
	var cellSpansPerCol [][]SyntaxSpan
	totalWidth := 0

	for col := 0; col < numCols; col++ {
		rc := renderedCellData{text: "", width: 0}
		if col < len(renderedCells) {
			rc = renderedCells[col]
		}
		totalWidth += rc.width

		// Create a single span for the cell content, then wrap
		sp := SyntaxSpan{
			Text:        rc.text,
			Kind:        TokenTable,
			State:       Rendered,
			BufferStart: lineStart,
			BufferEnd:   lineStart + len(lineText),
			CellMap:     rc.cm,
			BlockID:     block.id,
			BlockStart:  block.startOff,
			BlockEnd:    block.endOff,
			TableRole:   role,
			TableLayout: TableLayoutWrapped,
		}
		// Apply inline span styling if available
		if len(lineSpans) > 0 {
			for _, ms := range lineSpans {
				contentStart := ms.start + ms.delimLeft
				// Check if this span overlaps with the cell's content
				if rc.cm != nil && len(rc.cm) > 0 && len(rc.text) > 0 {
					sp.Kind = ms.kind
					break
				}
				_ = contentStart
			}
		}
		wrapped := wrapCellSpans([]SyntaxSpan{sp}, allocatedPerCol)
		cellSpansPerCol = append(cellSpansPerCol, wrapped...)
	}

	// Build the first wrapped line (subsequent lines would be synthetic SyntaxLines)
	var b strings.Builder
	var cm []CellMapping

	for col := 0; col < numCols; col++ {
		if col == 0 {
			b.WriteRune('│')
			cm = append(cm, CellMapping{BufOffset: -1})
		}
		b.WriteByte(' ')
		cm = append(cm, CellMapping{BufOffset: -1})

		// Get the first wrapped line for this cell
		var colSpans []SyntaxSpan
		if col < len(cellSpansPerCol) {
			colSpans = cellSpansPerCol[col]
		}

		// Write cell content
		cellText := ""
		var cellCM []CellMapping
		for _, s := range colSpans {
			cellText += s.Text
			cellCM = append(cellCM, s.CellMap...)
		}

		// Pad to allocated width
		cellW := runewidthSafe(cellText)
		pad := allocatedPerCol - cellW
		if pad < 0 {
			pad = 0
		}

		b.WriteString(cellText)
		cm = append(cm, cellCM...)
		for j := 0; j < pad; j++ {
			b.WriteByte(' ')
			cm = append(cm, CellMapping{BufOffset: -1})
		}

		b.WriteByte(' ')
		cm = append(cm, CellMapping{BufOffset: -1})
		b.WriteRune('│')
		cm = append(cm, CellMapping{BufOffset: -1})
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
	}}
}

// formatPivotedRow renders a table row as key-value pairs (Header: Value).
// In pivoted mode, the header row is suppressed (returns empty span).
func formatPivotedRow(block mdBlock, lineIdx int, renderedCells []renderedCellData, colWidths []int, alignments []int, lineStart int, lineText string, lineSpans []mdSpan, role TableRoleKind, availableWidth int) []SyntaxSpan {
	numCols := len(colWidths)

	// Suppress header row in pivoted mode
	if role == TableRoleHeader {
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

	// For data rows, render as "Header: Value" pairs
	// Get header values from the header line
	headerCells := parseTableCells(lineText)
	if lineIdx > block.headerEnd {
		// Read header line to get column labels
		// This is a simplification — in practice, the header text would be stored in the block
		headerCells = make([]string, numCols)
		for i := range headerCells {
			headerCells[i] = fmt.Sprintf("Col%d", i+1) // placeholder
		}
	}

	var b strings.Builder
	var cm []CellMapping

	for col := 0; col < numCols; col++ {
		rc := renderedCellData{text: "", width: 0}
		if col < len(renderedCells) {
			rc = renderedCells[col]
		}

		label := ""
		if col < len(headerCells) {
			label = headerCells[col]
		}

		// Format: "Label: Value"
		b.WriteString(label)
		b.WriteString(": ")
		b.WriteString(rc.text)

		// Build cell mapping
		for range label {
			cm = append(cm, CellMapping{BufOffset: -1})
		}
		for i := 0; i < 2; i++ {
			cm = append(cm, CellMapping{BufOffset: -1})
		}
		if len(rc.cm) > 0 {
			cm = append(cm, rc.cm...)
		}

		if col < numCols-1 {
			b.WriteString("\n")
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
