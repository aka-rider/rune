package display

import (
	"strings"
	"unicode/utf8"
)

// formatTableSeparatorSpansWithWidths creates separator spans using given column widths.
func formatTableSeparatorSpansWithWidths(colWidths []int, lineStart int, lineText string, block mdBlock) []SyntaxSpan {
	formatted := formatTableSeparator(colWidths)
	cm := make([]CellMapping, utf8.RuneCountInString(formatted))
	for i := range cm {
		cm[i] = CellMapping{BufOffset: -1}
	}
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
		TableRole:   TableRoleSeparator,
	}}
}

// tableLineRole determines whether a table line is header, separator, or body.
func tableLineRole(block mdBlock, lineIdx int) TableRoleKind {
	if lineIdx == block.sepLine {
		return TableRoleSeparator
	}
	if lineIdx <= block.headerEnd {
		return TableRoleHeader
	}
	return TableRoleBody
}

// computeTableMetrics scans table lines to compute max column widths and find the separator.
// Column widths are computed from rendered text (without markdown delimiters) when
// inline spans are available, falling back to raw source width otherwise.
// Also computes minimum column widths (longest unbreakable token per column).
func computeTableMetrics(lines []string, startLine, endLine int, parsed []parsedLine) (colWidths []int, minColWidths []int, sepLine int) {
	sepLine = -1
	for i := startLine; i <= endLine && i < len(lines); i++ {
		cells := parseTableCells(lines[i])
		if isSeparatorLine(lines[i]) {
			sepLine = i
			continue
		}
		var mdSpans []mdSpan
		if parsed != nil && i < len(parsed) {
			mdSpans = parsed[i].spans
		}
		cellOffsets := parseTableCellOffsets(lines[i])
		for col, cell := range cells {
			w := renderedCellWidth(cell, col, cellOffsets, mdSpans)
			if col >= len(colWidths) {
				colWidths = append(colWidths, w)
				minColWidths = append(minColWidths, 0)
			}
			if w > colWidths[col] {
				colWidths[col] = w
			}
			// Compute minimum width: longest unbreakable word/URL in this cell
			minW := longestAtomicWidth(cell)
			if col < len(minColWidths) && minW > minColWidths[col] {
				minColWidths[col] = minW
			}
		}
	}
	return colWidths, minColWidths, sepLine
}

// renderedCellWidth computes the visual width of a cell's rendered content.
// When inline spans are available, it reconstructs the rendered text (plain text
// gaps + span inner text) and measures that. Falls back to raw width with
// delimiter stripping when no spans cover the cell.
func renderedCellWidth(cellText string, colIdx int, cellOffsets []int, mdSpans []mdSpan) int {
	if len(mdSpans) > 0 && colIdx < len(cellOffsets) {
		cellStart := cellOffsets[colIdx]
		cellEnd := cellStart + len(cellText)

		// Collect spans belonging to this cell
		var cellSpans []mdSpan
		for _, ms := range mdSpans {
			if ms.start >= cellStart && ms.start < cellEnd {
				cellSpans = append(cellSpans, ms)
			}
		}

		if len(cellSpans) > 0 {
			// Reconstruct rendered text: plain text gaps + span inner text
			totalWidth := 0
			cursor := cellStart
			for _, s := range cellSpans {
				spanStart := s.start
				if spanStart > cursor {
					// Gap of plain text
					totalWidth += runewidthSafe(cellText[cursor-cellStart : spanStart-cellStart])
				}
				totalWidth += runewidthSafe(s.text)
				cursor = s.end
			}
			// Trailing plain text
			if cursor < cellEnd {
				totalWidth += runewidthSafe(cellText[cursor-cellStart:])
			}
			return totalWidth
		}
	}
	// Fallback: strip common markdown delimiters and measure
	return cellWidth(stripDelimiters(cellText))
}

// stripDelimiters removes common markdown formatting delimiters for width estimation.
func stripDelimiters(s string) string {
	for _, delim := range []string{"**", "*", "_", "~~"} {
		s = strings.ReplaceAll(s, delim, "")
	}
	s = strings.Trim(s, "`")
	if idx := strings.Index(s, "]("); idx >= 0 {
		text := s[:idx]
		s = text + s[idx+2:]
	}
	s = strings.ReplaceAll(s, "[", "")
	s = strings.ReplaceAll(s, "]", "")
	s = strings.ReplaceAll(s, "(", "")
	s = strings.ReplaceAll(s, ")", "")
	return s
}

// parseTableCells splits a pipe-delimited table line into cell contents.
func parseTableCells(line string) []string {
	trimmed := strings.TrimSpace(line)
	// Strip leading/trailing pipes
	if len(trimmed) > 0 && trimmed[0] == '|' {
		trimmed = trimmed[1:]
	}
	if len(trimmed) > 0 && trimmed[len(trimmed)-1] == '|' {
		trimmed = trimmed[:len(trimmed)-1]
	}
	parts := strings.Split(trimmed, "|")
	cells := make([]string, len(parts))
	for i, p := range parts {
		cells[i] = strings.TrimSpace(p)
	}
	return cells
}

// isSeparatorLine checks if a line is a table separator (e.g., |---|---|).
func isSeparatorLine(line string) bool {
	trimmed := strings.TrimSpace(line)
	if len(trimmed) == 0 {
		return false
	}
	for _, ch := range trimmed {
		if ch != '|' && ch != '-' && ch != ':' && ch != ' ' {
			return false
		}
	}
	// Must contain at least one dash
	return strings.Contains(trimmed, "-")
}

// cellWidth returns the visual width of cell content.
func cellWidth(cell string) int {
	return runewidthSafe(cell)
}

// longestAtomicWidth returns the visual width of the longest unbreakable token
// in a cell. URLs (http://, https://) are atomic; other text splits at spaces.
func longestAtomicWidth(cell string) int {
	cell = strings.TrimSpace(cell)
	// Strip common markdown delimiters for measurement
	cell = stripDelimiters(cell)
	words := strings.Fields(cell)
	maxW := 0
	for _, word := range words {
		w := runewidthSafe(word)
		if w > maxW {
			maxW = w
		}
	}
	return maxW
}

// runewidthSafe returns the visual width of a string.
func runewidthSafe(s string) int {
	w := 0
	for _, r := range s {
		w += runeWidth(r)
	}
	return w
}

// runeWidth returns the visual width of a rune (2 for CJK, 1 otherwise).
func runeWidth(r rune) int {
	if r >= 0x1100 && isWide(r) {
		return 2
	}
	return 1
}

// isWide checks if a rune is East Asian wide.
func isWide(r rune) bool {
	return (r >= 0x1100 && r <= 0x115F) ||
		(r >= 0x2E80 && r <= 0x9FFF) ||
		(r >= 0xAC00 && r <= 0xD7AF) ||
		(r >= 0xF900 && r <= 0xFAFF) ||
		(r >= 0xFE10 && r <= 0xFE6F) ||
		(r >= 0xFF01 && r <= 0xFF60) ||
		(r >= 0xFFE0 && r <= 0xFFE6) ||
		(r >= 0x20000 && r <= 0x2FFFD) ||
		(r >= 0x30000 && r <= 0x3FFFD)
}

// separatorType determines which corner/junction characters to use for separators.
type separatorType int

const (
	separatorHeaderBody separatorType = iota
	separatorTop
	separatorBottom
	separatorBody
)

// formatTableSeparator creates a formatted separator line using box-drawing characters.
func formatTableSeparator(colWidths []int) string {
	return formatTableSeparatorWithType(colWidths, separatorHeaderBody)
}

// formatTableSeparatorWithType creates a separator with specific corner/junction characters.
func formatTableSeparatorWithType(colWidths []int, sepType separatorType) string {
	var b strings.Builder

	var leftCorner, junction, rightCorner, horiz rune
	switch sepType {
	case separatorTop:
		leftCorner, junction, rightCorner, horiz = '┌', '┬', '┐', '─'
	case separatorBottom:
		leftCorner, junction, rightCorner, horiz = '└', '┴', '┘', '─'
	case separatorBody:
		leftCorner, junction, rightCorner, horiz = '├', '┼', '┤', '─'
	default: // separatorHeaderBody
		leftCorner, junction, rightCorner, horiz = '├', '┼', '┤', '─'
	}

	for i, w := range colWidths {
		if i == 0 {
			b.WriteRune(leftCorner)
		} else {
			b.WriteRune(junction)
		}
		b.WriteRune(horiz)
		for j := 0; j < w; j++ {
			b.WriteRune(horiz)
		}
		b.WriteRune(horiz)
	}
	b.WriteRune(rightCorner)
	return b.String()
}

// formatTableRow formats a pipe-delimited row with padded cells.
// Returns the formatted string and a CellMap mapping each output visual cell to a buffer offset.
// NOTE: This function is kept for backward compatibility. New code should use
// formatTableRowRendered for span-aware rendering.
func formatTableRow(line string, colWidths []int, alignments []int, lineStart int) (string, []CellMapping) {
	cells := parseTableCells(line)
	cellOffsets := parseTableCellOffsets(line)

	var b strings.Builder
	var cm []CellMapping
	for i, w := range colWidths {
		if i == 0 {
			b.WriteByte('|')
			cm = append(cm, CellMapping{BufOffset: -1})
		}
		b.WriteByte(' ')
		cm = append(cm, CellMapping{BufOffset: -1})

		cell := ""
		if i < len(cells) {
			cell = cells[i]
		}

		cw := cellWidth(cell)
		pad := w - cw
		if pad < 0 {
			pad = 0
		}

		align := 0 // left
		if i < len(alignments) {
			align = alignments[i]
		}

		cellBufStart := -1
		if i < len(cellOffsets) {
			cellBufStart = lineStart + cellOffsets[i]
		}

		switch align {
		case 2: // right
			for j := 0; j < pad; j++ {
				b.WriteByte(' ')
				cm = append(cm, CellMapping{BufOffset: -1})
			}
			writeCellWithMap(&b, &cm, cell, cellBufStart)
		case 1: // center
			leftPad := pad / 2
			rightPad := pad - leftPad
			for j := 0; j < leftPad; j++ {
				b.WriteByte(' ')
				cm = append(cm, CellMapping{BufOffset: -1})
			}
			writeCellWithMap(&b, &cm, cell, cellBufStart)
			for j := 0; j < rightPad; j++ {
				b.WriteByte(' ')
				cm = append(cm, CellMapping{BufOffset: -1})
			}
		default: // left
			writeCellWithMap(&b, &cm, cell, cellBufStart)
			for j := 0; j < pad; j++ {
				b.WriteByte(' ')
				cm = append(cm, CellMapping{BufOffset: -1})
			}
		}

		b.WriteByte(' ')
		cm = append(cm, CellMapping{BufOffset: -1})
		b.WriteByte('|')
		cm = append(cm, CellMapping{BufOffset: -1})
	}
	return b.String(), cm
}

// writeCellWithMap writes cell content to the builder and appends CellMapping entries.
// CellMap is built per-visual-cell (per-rune), not per-byte.
func writeCellWithMap(b *strings.Builder, cm *[]CellMapping, cell string, bufStart int) {
	for pos := 0; pos < len(cell); {
		b.WriteRune(rune(cell[pos]))
		off := -1
		if bufStart >= 0 {
			off = bufStart + pos
		}
		*cm = append(*cm, CellMapping{BufOffset: off})
		_, size := utf8.DecodeRuneInString(cell[pos:])
		if size == 0 {
			size = 1
		}
		pos += size
	}
}

// parseTableCellOffsets returns the byte offset within the line where each cell's
// trimmed content begins. This allows mapping formatted cell chars back to source.
func parseTableCellOffsets(line string) []int {
	trimmed := strings.TrimSpace(line)
	baseOffset := strings.Index(line, trimmed)
	if baseOffset < 0 {
		baseOffset = 0
	}

	// Strip leading pipe
	inner := trimmed
	innerOffset := baseOffset
	if len(inner) > 0 && inner[0] == '|' {
		inner = inner[1:]
		innerOffset++
	}
	// Strip trailing pipe
	if len(inner) > 0 && inner[len(inner)-1] == '|' {
		inner = inner[:len(inner)-1]
	}

	var offsets []int
	pos := 0
	for _, part := range strings.Split(inner, "|") {
		trimmedPart := strings.TrimSpace(part)
		contentOff := 0
		if trimmedPart != "" {
			contentOff = strings.Index(part, trimmedPart)
		} else {
			contentOff = len(part)
		}
		offsets = append(offsets, innerOffset+pos+contentOff)
		pos += len(part) + 1 // +1 for the '|' separator
	}
	return offsets
}
