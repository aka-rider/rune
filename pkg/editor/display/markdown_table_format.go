package display

import (
	"sort"
	"strings"
	"unicode/utf8"
)

// extractRenderedCellData extracts rendered text and cell mappings for each cell.
// It iterates left-to-right through each cell's byte range, emitting plain text
// for gaps between inline spans and rendered (delimiter-stripped) text for spans.
// This produces the full rendered cell content with correct per-byte buffer mappings.
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
			// A span belongs to this cell if it overlaps the cell's byte range.
			// Use the full span range (start..end) to check overlap.
			if s.start >= cellRelStart && s.start < cellRelEnd {
				cellSpans = append(cellSpans, s)
			}
		}

		if len(cellSpans) > 0 {
			// Sort spans by start position
			sorted := make([]mdSpan, len(cellSpans))
			copy(sorted, cellSpans)
			sort.Slice(sorted, func(i, j int) bool {
				return sorted[i].start < sorted[j].start
			})

			// Walk left-to-right through the cell, emitting plain text for gaps
			// and rendered span text for formatted ranges.
			var renderedText strings.Builder
			var cm []CellMapping
			cursor := cellRelStart

			for _, s := range sorted {
				spanStart := s.start
				spanEnd := s.end

				// Clamp to cell boundaries
				if spanStart < cellRelStart {
					spanStart = cellRelStart
				}
				if spanEnd > cellRelEnd {
					spanEnd = cellRelEnd
				}

				// Emit plain text gap before this span
				if cursor < spanStart {
					gap := lineText[cursor:spanStart]
					renderedText.WriteString(gap)
					for i := 0; i < len(gap); i++ {
						cm = append(cm, CellMapping{BufOffset: lineStart + cursor + i})
					}
					cursor = spanStart
				}

				// Emit the span's rendered text (inner text, without delimiters)
				renderedText.WriteString(s.text)
				contentStart := lineStart + s.start + s.delimLeft
				for i := 0; i < len(s.text); i++ {
					cm = append(cm, CellMapping{BufOffset: contentStart + i})
				}

				// Advance cursor past the entire span (including delimiters)
				cursor = s.end
			}

			// Emit trailing plain text after the last span
			if cursor < cellRelEnd {
				gap := lineText[cursor:cellRelEnd]
				renderedText.WriteString(gap)
				for i := 0; i < len(gap); i++ {
					cm = append(cm, CellMapping{BufOffset: lineStart + cursor + i})
				}
			}

			text := renderedText.String()
			result[col] = renderedCellData{
				text:  text,
				cm:    cm,
				width: runewidthSafe(text),
			}
		} else {
			// No spans — use raw cell text as-is (plain text, no formatting)
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

// formatTableRowRendered formats a table row using pre-extracted rendered cell data.
// This avoids the mismatch between raw source text (with delimiters) and rendered text.
// Uses box-drawing vertical characters (│) for borders.
// Cell content is truncated with "…" if it exceeds the allocated column width.
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

		// Get rendered cell data, truncating if necessary
		rc := renderedCellData{text: "", width: 0}
		if i < len(renderedCells) {
			rc = renderedCells[i]
		}

		// Truncate cell content if it exceeds the allocated column width
		if rc.width > w && w > 0 {
			rc = truncateCellData(rc, w)
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

// truncateCellData truncates cell content to fit within maxWidth visual columns,
// appending "…" (ellipsis) if truncation occurs.
func truncateCellData(rc renderedCellData, maxWidth int) renderedCellData {
	if maxWidth <= 0 {
		return renderedCellData{text: "", width: 0}
	}
	if rc.width <= maxWidth {
		return rc
	}

	// Reserve 1 cell for the ellipsis "…" (3 bytes UTF-8, 1 visual width)
	targetWidth := maxWidth - 1
	if targetWidth < 0 {
		targetWidth = 0
	}

	// Walk through text rune by rune, accumulating visual width
	var truncText strings.Builder
	var truncCM []CellMapping
	accWidth := 0
	pos := 0
	cmIdx := 0

	for pos < len(rc.text) {
		r, size := utf8.DecodeRuneInString(rc.text[pos:])
		rw := 1
		if r >= 0x1100 && isWide(r) {
			rw = 2
		}
		if accWidth+rw > targetWidth {
			break
		}
		truncText.WriteRune(r)
		// Advance CellMap by byte count
		for j := 0; j < size && cmIdx < len(rc.cm); j++ {
			truncCM = append(truncCM, rc.cm[cmIdx])
			cmIdx++
		}
		accWidth += rw
		pos += size
	}

	// Append ellipsis
	truncText.WriteRune('…')
	// "…" is 3 bytes — add 3 CellMapping entries with -1
	truncCM = append(truncCM, CellMapping{BufOffset: -1}, CellMapping{BufOffset: -1}, CellMapping{BufOffset: -1})

	return renderedCellData{
		text:  truncText.String(),
		cm:    truncCM,
		width: accWidth + 1, // +1 for the ellipsis
	}
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
		kind TokenKind
		sp   *mdSpan
		cm   CellMapping
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
