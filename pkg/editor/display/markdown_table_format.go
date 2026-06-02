package display

import (
	"sort"
	"strings"
)

// extractRenderedCellData extracts rendered text and cell mappings for each cell.
// When spans are available, it uses span text (inner text without delimiters).
// Falls back to raw cell text when no spans cover a cell.
func extractRenderedCellData(lineText string, lineStart int, spans []mdSpan, numCols int) []renderedCellData {
	cells := parseTableCells(lineText)
	cellOffsets := parseTableCellOffsets(lineText)

	result := make([]renderedCellData, numCols)

	for col := 0; col < numCols; col++ {
		cellText := ""
		if col < len(cells) {
			cellText = cells[col]
		}

		// Find the byte range for this cell's content within the line
		cellRelStart := -1
		if col < len(cellOffsets) {
			cellRelStart = cellOffsets[col]
		}
		cellRelEnd := cellRelStart + len(cellText)
		if cellRelStart < 0 {
			cellRelEnd = 0
		}

		// Collect spans that belong to this cell
		var cellSpans []mdSpan
		for _, s := range spans {
			// A span belongs to this cell if its content start (after left delimiter)
			// falls within the cell's byte range
			contentStart := s.start + s.delimLeft
			if contentStart >= cellRelStart && contentStart < cellRelEnd {
				cellSpans = append(cellSpans, s)
			}
		}

		if len(cellSpans) > 0 {
			// Use rendered text from spans
			var totalWidth int
			var renderedText strings.Builder
			var cm []CellMapping

			// Sort spans by start position
			sorted := make([]mdSpan, len(cellSpans))
			copy(sorted, cellSpans)
			sort.Slice(sorted, func(i, j int) bool {
				return sorted[i].start < sorted[j].start
			})

			for _, s := range sorted {
				renderedText.WriteString(s.text)
				totalWidth += runewidthSafe(s.text)
				// Build cell mapping: each byte of rendered text maps to its buffer offset
				bufStart := lineStart + s.start + s.delimLeft
				for i := 0; i < len(s.text); i++ {
					cm = append(cm, CellMapping{BufOffset: bufStart + i})
				}
			}

			result[col] = renderedCellData{
				text:  renderedText.String(),
				cm:    cm,
				width: totalWidth,
			}
		} else {
			// Fall back to raw cell text (no spans, no delimiter stripping needed)
			w := cellWidth(cellText)
			cm := make([]CellMapping, len(cellText))
			bufStart := -1
			if col < len(cellOffsets) {
				bufStart = lineStart + cellOffsets[col]
			}
			for i := range cm {
				off := -1
				if bufStart >= 0 {
					off = bufStart + i
				}
				cm[i] = CellMapping{BufOffset: off}
			}
			result[col] = renderedCellData{
				text:  cellText,
				cm:    cm,
				width: w,
			}
		}
	}

	return result
}

// computeRenderedColWidths computes column widths from rendered cell data.
// Returns the maximum rendered width across all cells for each column.
func computeRenderedColWidths(renderedCells []renderedCellData, fallbackWidths []int) []int {
	if len(renderedCells) == 0 {
		return fallbackWidths
	}

	widths := make([]int, len(fallbackWidths))
	copy(widths, fallbackWidths)

	for col, rc := range renderedCells {
		if col < len(widths) {
			// Use the larger of rendered width and fallback (to handle cases where
			// other rows have wider raw content)
			if rc.width > widths[col] {
				widths[col] = rc.width
			}
		}
	}

	// Adjust widths down: subtract the total delimiter width per column
	// since rendered text doesn't include delimiters
	for col := range widths {
		if col < len(renderedCells) {
			rc := renderedCells[col]
			if rc.width < widths[col] {
				widths[col] = rc.width
			}
		}
	}

	return widths
}

// formatTableRowRendered formats a table row using pre-extracted rendered cell data.
// This avoids the mismatch between raw source text (with delimiters) and rendered text.
// Uses box-drawing vertical characters (│) for borders.
func formatTableRowRendered(renderedCells []renderedCellData, colWidths []int, alignments []int, lineStart int, lineText string) (string, []CellMapping) {
	var b strings.Builder
	var cm []CellMapping

	for i, w := range colWidths {
		// Opening vertical border (│ is 3 bytes in UTF-8 — one CellMapping per byte)
		if i == 0 {
			b.WriteRune('│')
			cm = append(cm, CellMapping{BufOffset: -1}, CellMapping{BufOffset: -1}, CellMapping{BufOffset: -1})
		}
		// Left padding space
		b.WriteByte(' ')
		cm = append(cm, CellMapping{BufOffset: -1})

		// Get rendered cell data
		rc := renderedCellData{text: "", width: 0}
		if i < len(renderedCells) {
			rc = renderedCells[i]
		}

		cw := rc.width
		pad := w - cw
		if pad < 0 {
			pad = 0
		}

		align := 0 // left
		if i < len(alignments) {
			align = alignments[i]
		}

		switch align {
		case 2: // right
			for j := 0; j < pad; j++ {
				b.WriteByte(' ')
				cm = append(cm, CellMapping{BufOffset: -1})
			}
			writeRenderedCell(&b, &cm, rc)
		case 1: // center
			leftPad := pad / 2
			rightPad := pad - leftPad
			for j := 0; j < leftPad; j++ {
				b.WriteByte(' ')
				cm = append(cm, CellMapping{BufOffset: -1})
			}
			writeRenderedCell(&b, &cm, rc)
			for j := 0; j < rightPad; j++ {
				b.WriteByte(' ')
				cm = append(cm, CellMapping{BufOffset: -1})
			}
		default: // left
			writeRenderedCell(&b, &cm, rc)
			for j := 0; j < pad; j++ {
				b.WriteByte(' ')
				cm = append(cm, CellMapping{BufOffset: -1})
			}
		}

		// Right padding space and closing vertical border (│ is 3 bytes — one CellMapping per byte)
		b.WriteByte(' ')
		cm = append(cm, CellMapping{BufOffset: -1})
		b.WriteRune('│')
		cm = append(cm, CellMapping{BufOffset: -1}, CellMapping{BufOffset: -1}, CellMapping{BufOffset: -1})
	}

	return b.String(), cm
}

// writeRenderedCell writes rendered cell content to the builder using pre-computed cell mapping.
func writeRenderedCell(b *strings.Builder, cm *[]CellMapping, rc renderedCellData) {
	if len(rc.cm) > 0 {
		*cm = append(*cm, rc.cm...)
	} else {
		for i := 0; i < len(rc.text); i++ {
			*cm = append(*cm, CellMapping{BufOffset: -1})
		}
	}
	b.WriteString(rc.text)
}

// buildTableStyledSpans creates styled SyntaxSpan entries from inline span data.
// Each span in the formatted output gets the correct TokenKind based on the inline element.
func buildTableStyledSpans(block mdBlock, lineIdx int, formatted string, cm []CellMapping, lineStart int, lineText string, spans []mdSpan, role TableRoleKind) []SyntaxSpan {
	if len(spans) == 0 || len(cm) == 0 {
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
		}}
	}

	// Sort spans by start position
	sorted := make([]mdSpan, len(spans))
	copy(sorted, spans)
	sort.Slice(sorted, func(i, j int) bool {
		return sorted[i].start < sorted[j].start
	})

	// Build per-byte kind classification
	type byteInfo struct {
		kind  TokenKind
		sp    *mdSpan
		cm    CellMapping
	}

	byteInfos := make([]byteInfo, len(formatted))

	for i, cellMap := range cm {
		bufOff := cellMap.BufOffset
		kind := TokenTable
		var activeSpan *mdSpan

		if bufOff >= 0 {
			relOff := bufOff - lineStart

			// Find the innermost span that covers this offset
			// Exclude delimiter bytes from span styling
			for _, ms := range sorted {
				contentStart := ms.start + ms.delimLeft
				contentEnd := ms.end - ms.delimRight
				if relOff >= contentStart && relOff < contentEnd {
					kind = ms.kind
					spanCopy := ms
					activeSpan = &spanCopy
					break
				}
			}
		}

		byteInfos[i] = byteInfo{kind: kind, sp: activeSpan, cm: cellMap}
	}

	// Group consecutive bytes with the same kind into spans
	var result []SyntaxSpan
	var currentText strings.Builder
	var currentCM []CellMapping
	var currentKind TokenKind
	var currentActiveSpan *mdSpan

	flushSpan := func() {
		if currentText.Len() > 0 {
			sp := SyntaxSpan{
				Text:        currentText.String(),
				Kind:        currentKind,
				State:       Rendered,
				BufferStart: lineStart,
				BufferEnd:   lineStart + len(lineText),
				CellMap:     currentCM,
				BlockID:     block.id,
				BlockStart:  block.startOff,
				BlockEnd:    block.endOff,
				TableRole:   role,
			}
			if currentActiveSpan != nil {
				sp.AltText = spanAltText(*currentActiveSpan)
				sp.ImagePath = spanImagePath(*currentActiveSpan)
				sp.EmbedRef = spanEmbedRef(*currentActiveSpan)
				sp.CalloutKind = spanCalloutKind(*currentActiveSpan)
				sp.HeadingLevel = currentActiveSpan.level
				sp.WikiLinkTarget = spanWikiLinkTarget(*currentActiveSpan)
				sp.WikiLinkIsImage = spanWikiLinkIsImage(*currentActiveSpan)
				sp.LinkURL = spanLinkURL(*currentActiveSpan)
			}
			result = append(result, sp)
			currentText.Reset()
			currentCM = nil
		}
	}

	for i := 0; i < len(byteInfos); i++ {
		bi := byteInfos[i]
		kindChanged := bi.kind != currentKind
		spanChanged := (bi.sp == nil && currentActiveSpan != nil) ||
			(bi.sp != nil && currentActiveSpan == nil) ||
			(bi.sp != nil && currentActiveSpan != nil && bi.sp.kind != currentActiveSpan.kind)

		if kindChanged || spanChanged {
			flushSpan()
			currentKind = bi.kind
			currentActiveSpan = bi.sp
		}

		currentText.WriteByte(formatted[i])
		currentCM = append(currentCM, bi.cm)
	}
	flushSpan()

	return result
}

