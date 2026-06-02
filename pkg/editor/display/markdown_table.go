package display

import (
	"sort"
	"strings"

	"github.com/mattn/go-runewidth"
)

// cellContent holds the rendered spans and width for a single table cell.
type cellContent struct {
	spans []SyntaxSpan
	width int
}

// tableRenderedSpans produces spans for a table line in rendered mode.
// It formats cells with padding and alignment, and identifies line roles.
// When inline spans are available, it renders cell content from span texts
// (rendered text without markdown delimiters) with correct span kinds.
func tableRenderedSpans(block mdBlock, lineIdx int, lineText string, lineStart int, mdSpans []mdSpan) []SyntaxSpan {
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

	// For separator lines, render a formatted separator
	if role == TableRoleSeparator {
		return formatTableSeparatorSpans(block, lineStart, lineText)
	}

	// For header and body lines, format cells with styled spans
	return formatStyledTableRow(block, lineIdx, lineText, lineStart, mdSpans, role)
}

// formatStyledTableRow formats a table row with styled inline spans.
// When mdSpans are available, it renders cell content from span texts (without delimiters).
// Falls back to raw source formatting when no spans exist.
func formatStyledTableRow(block mdBlock, lineIdx int, lineText string, lineStart int, mdSpans []mdSpan, role TableRoleKind) []SyntaxSpan {
	cells := parseTableCells(lineText)
	cellOffsets := parseTableCellOffsets(lineText)

	// Sort inline spans by start offset
	sorted := make([]mdSpan, len(mdSpans))
	copy(sorted, mdSpans)
	sort.Slice(sorted, func(i, j int) bool {
		return sorted[i].start < sorted[j].start
	})

	// Build rendered cell content from spans or raw text
	cellContents := make([]cellContent, len(block.colWidths))

	for col := range block.colWidths {
		cellText := ""
		if col < len(cells) {
			cellText = cells[col]
		}

		// Find the byte range of this cell's content in the line
		cellRelStart := -1
		if col < len(cellOffsets) {
			cellRelStart = cellOffsets[col]
		}
		cellRelEnd := cellRelStart + len(cellText)

		// Collect spans that belong to this cell
		var cellSpans []mdSpan
		for _, ms := range sorted {
			if ms.start >= cellRelStart && ms.end <= cellRelEnd {
				cellSpans = append(cellSpans, ms)
			}
		}

		if len(cellSpans) > 0 {
			// Render from spans — use rendered text without delimiters
			cellContents[col] = renderCellFromSpans(cellSpans, lineStart, cellRelStart, block, role)
		} else if cellText != "" {
			// No spans — use raw cell text (fallback for plain text cells)
			cellContents[col] = cellContent{
				spans: []SyntaxSpan{makePlainCellSpan(cellText, lineStart, cellRelStart, block, role)},
				width: cellWidth(cellText),
			}
		}
	}

	// Compose the row: | cell1 | cell2 | ... |
	// Each cell is padded to its column width
	var result []SyntaxSpan
	for col, w := range block.colWidths {
		// Left border
		result = append(result, SyntaxSpan{
			Text:        "| ",
			Kind:        TokenTable,
			State:       Rendered,
			BufferStart: lineStart,
			BufferEnd:   lineStart + len(lineText),
			CellMap:     []CellMapping{{BufOffset: -1}, {BufOffset: -1}},
			BlockID:     block.id,
			BlockStart:  block.startOff,
			BlockEnd:    block.endOff,
			TableRole:   role,
		})

		align := 0
		if col < len(block.alignments) {
			align = block.alignments[col]
		}

		cc := cellContents[col]
		pad := w - cc.width
		if pad < 0 {
			pad = 0
		}

		// Left padding (for right/center aligned)
		leftPad := 0
		if align == 2 { // right
			leftPad = pad
		} else if align == 1 { // center
			leftPad = pad / 2
		}
		if leftPad > 0 {
			result = append(result, makePaddingSpan(leftPad, lineStart, len(lineText), block, role))
		}

		// Cell content spans
		result = append(result, cc.spans...)

		// Right padding
		rightPad := pad - leftPad
		if rightPad > 0 {
			result = append(result, makePaddingSpan(rightPad, lineStart, len(lineText), block, role))
		}

		// Right border
		result = append(result, SyntaxSpan{
			Text:        " |",
			Kind:        TokenTable,
			State:       Rendered,
			BufferStart: lineStart,
			BufferEnd:   lineStart + len(lineText),
			CellMap:     []CellMapping{{BufOffset: -1}, {BufOffset: -1}},
			BlockID:     block.id,
			BlockStart:  block.startOff,
			BlockEnd:    block.endOff,
			TableRole:   role,
		})
	}

	return result
}

// renderCellFromSpans produces styled SyntaxSpans from a cell's inline mdSpans.
func renderCellFromSpans(cellSpans []mdSpan, lineStart int, cellRelStart int, block mdBlock, role TableRoleKind) cellContent {
	// Sort spans by start
	sort.Slice(cellSpans, func(i, j int) bool {
		return cellSpans[i].start < cellSpans[j].start
	})

	var spans []SyntaxSpan
	totalWidth := 0
	pos := 0 // position within cell content

	for _, ms := range cellSpans {
		// Gap between spans (plain text between formatted elements)
		if ms.start > pos {
			gapLen := ms.start - pos
			if gapLen > 0 {
				// Use spaces for gaps (unrendered whitespace between spans)
				var b strings.Builder
				for i := 0; i < gapLen; i++ {
					b.WriteByte(' ')
				}
				gapCM := make([]CellMapping, gapLen)
				for i := range gapCM {
					gapCM[i] = CellMapping{BufOffset: lineStart + pos + i}
				}
				spans = append(spans, SyntaxSpan{
					Text:        b.String(),
					Kind:        TokenTable,
					State:       Rendered,
					BufferStart: lineStart,
					BufferEnd:   lineStart + gapLen,
					CellMap:     gapCM,
					BlockID:     block.id,
					BlockStart:  block.startOff,
					BlockEnd:    block.endOff,
					TableRole:   role,
				})
			}
		}

		// Render the span text
		spText := ms.text
		spWidth := runewidth.StringWidth(spText)

		cm := buildInlineCellMap(lineStart+ms.start, len(spText))
		syntaxSpan := SyntaxSpan{
			Text:        spText,
			Kind:        ms.kind,
			State:       Rendered,
			BufferStart: lineStart,
			BufferEnd:   lineStart + len(spText),
			CellMap:     cm,
			BlockID:     block.id,
			BlockStart:  block.startOff,
			BlockEnd:    block.endOff,
			TableRole:   role,
		}
		syntaxSpan.AltText = spanAltText(ms)
		syntaxSpan.ImagePath = spanImagePath(ms)
		syntaxSpan.EmbedRef = spanEmbedRef(ms)
		syntaxSpan.CalloutKind = spanCalloutKind(ms)
		syntaxSpan.HeadingLevel = ms.level
		syntaxSpan.WikiLinkTarget = spanWikiLinkTarget(ms)
		syntaxSpan.WikiLinkIsImage = spanWikiLinkIsImage(ms)
		syntaxSpan.LinkURL = spanLinkURL(ms)

		spans = append(spans, syntaxSpan)
		totalWidth += spWidth
		pos = ms.end - cellRelStart
	}

	return cellContent{spans: spans, width: totalWidth}
}

// makePlainCellSpan creates a plain TokenTable span for unstyled cell content.
func makePlainCellSpan(text string, lineStart int, cellRelStart int, block mdBlock, role TableRoleKind) SyntaxSpan {
	cm := make([]CellMapping, len(text))
	for i := range cm {
		cm[i] = CellMapping{BufOffset: lineStart + cellRelStart + i}
	}
	return SyntaxSpan{
		Text:        text,
		Kind:        TokenTable,
		State:       Rendered,
		BufferStart: lineStart,
		BufferEnd:   lineStart + len(text),
		CellMap:     cm,
		BlockID:     block.id,
		BlockStart:  block.startOff,
		BlockEnd:    block.endOff,
		TableRole:   role,
	}
}

// makePaddingSpan creates a span of space characters for cell padding.
func makePaddingSpan(count int, lineStart int, lineLen int, block mdBlock, role TableRoleKind) SyntaxSpan {
	var b strings.Builder
	for i := 0; i < count; i++ {
		b.WriteByte(' ')
	}
	cm := make([]CellMapping, count)
	for i := range cm {
		cm[i] = CellMapping{BufOffset: -1}
	}
	return SyntaxSpan{
		Text:        b.String(),
		Kind:        TokenTable,
		State:       Rendered,
		BufferStart: lineStart,
		BufferEnd:   lineStart + lineLen,
		CellMap:     cm,
		BlockID:     block.id,
		BlockStart:  block.startOff,
		BlockEnd:    block.endOff,
		TableRole:   role,
	}
}

// formatTableSeparatorSpans produces styled spans for a table separator line.
func formatTableSeparatorSpans(block mdBlock, lineStart int, lineText string) []SyntaxSpan {
	formatted := formatTableSeparator(block.colWidths)
	cm := make([]CellMapping, len(formatted))
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
func computeTableMetrics(lines []string, startLine, endLine int, parsed []parsedLine) (colWidths []int, sepLine int) {
	sepLine = -1
	for i := startLine; i <= endLine && i < len(lines); i++ {
		cells := parseTableCells(lines[i])
		if isSeparatorLine(lines[i]) {
			sepLine = i
			continue
		}
		var mdSpans []mdSpan
		if i < len(parsed) {
			mdSpans = parsed[i].spans
		}
		for col, cell := range cells {
			w := renderedCellWidth(cell, lines[i], mdSpans)
			if col >= len(colWidths) {
				colWidths = append(colWidths, w)
			} else if w > colWidths[col] {
				colWidths[col] = w
			}
		}
	}
	return colWidths, sepLine
}

// renderedCellWidth computes the visual width of a cell's rendered content.
// When inline spans are available, it sums the rendered text widths (without delimiters).
// Falls back to raw source width (with delimiter stripping) when no spans exist.
func renderedCellWidth(cellText string, lineText string, mdSpans []mdSpan) int {
	if len(mdSpans) > 0 {
		// Find spans that belong to this cell's content and sum rendered widths
		cellStart := strings.Index(lineText, cellText)
		if cellStart >= 0 {
			cellEnd := cellStart + len(cellText)
			total := 0
			for _, ms := range mdSpans {
				if ms.start >= cellStart && ms.end <= cellEnd {
					total += runewidth.StringWidth(ms.text)
				}
			}
			if total > 0 {
				return total
			}
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
	// Strip inline code backticks
	s = strings.Trim(s, "`")
	// Strip link brackets and parens: [text](url) → text
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
	w := 0
	for _, r := range cell {
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
	// Simplified check for common CJK ranges
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

// formatTableSeparator creates a formatted separator line using box-drawing characters.
// Produces: ├────┼────┤
func formatTableSeparator(colWidths []int) string {
	var b strings.Builder
	for i, w := range colWidths {
		if i == 0 {
			b.WriteRune('├')
		} else {
			b.WriteRune('┼')
		}
		b.WriteRune(' ')
		for j := 0; j < w; j++ {
			b.WriteRune('─')
		}
		b.WriteRune(' ')
		b.WriteRune('┤')
	}
	return b.String()
}

// formatTableRow formats a pipe-delimited row with padded cells.
// Returns the formatted string and a CellMap mapping each output byte to a buffer offset.
func formatTableRow(line string, colWidths []int, alignments []int, lineStart int) (string, []CellMapping) {
	cells := parseTableCells(line)

	// Build a mapping from each cell's content to its byte offset within the source line.
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

		// Determine the buffer offset for this cell's content
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
func writeCellWithMap(b *strings.Builder, cm *[]CellMapping, cell string, bufStart int) {
	for i := 0; i < len(cell); i++ {
		b.WriteByte(cell[i])
		off := -1
		if bufStart >= 0 {
			off = bufStart + i
		}
		*cm = append(*cm, CellMapping{BufOffset: off})
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
		// Find the trimmed content start within this part
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
