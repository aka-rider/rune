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

				// Skip spans entirely before the cursor (already covered by a previous span).
				// This handles nested inline elements (e.g., italic inside bold) where the
				// outer span's text already includes the inner span's rendered text.
				if spanEnd <= cursor {
					continue
				}

				// Emit plain text gap before this span
				if cursor < spanStart {
					gap := lineText[cursor:spanStart]
					renderedText.WriteString(gap)
					for pos := 0; pos < len(gap); {
						cm = append(cm, CellMapping{BufOffset: lineStart + cursor})
						_, size := utf8.DecodeRuneInString(gap[pos:])
						if size == 0 {
							size = 1
						}
						pos += size
						cursor++
					}
				}

				// Emit the span's rendered text (inner text, without delimiters)
				renderedText.WriteString(s.text)
				contentStart := lineStart + s.start + s.delimLeft
				for pos := 0; pos < len(s.text); {
					cm = append(cm, CellMapping{BufOffset: contentStart + pos})
					_, size := utf8.DecodeRuneInString(s.text[pos:])
					if size == 0 {
						size = 1
					}
					pos += size
				}

				// Advance cursor past the entire span (including delimiters)
				cursor = s.end
			}

			// Emit trailing plain text after the last span
			if cursor < cellRelEnd {
				gap := lineText[cursor:cellRelEnd]
				renderedText.WriteString(gap)
				for pos := 0; pos < len(gap); {
					cm = append(cm, CellMapping{BufOffset: lineStart + cursor})
					_, size := utf8.DecodeRuneInString(gap[pos:])
					if size == 0 {
						size = 1
					}
					pos += size
					cursor++
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
			bufStart := -1
			if col < len(cellOffsets) {
				bufStart = lineStart + cellOffsets[col]
			}
			var cm []CellMapping
			for pos := 0; pos < len(cellText); {
				off := -1
				if bufStart >= 0 {
					off = bufStart + pos
				}
				cm = append(cm, CellMapping{BufOffset: off})
				_, size := utf8.DecodeRuneInString(cellText[pos:])
				if size == 0 {
					size = 1
				}
				pos += size
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
func formatTableRowRendered(renderedCells []renderedCellData, colWidths []int, alignments []int, lineStart int, lineText string) (string, []CellMapping) {
	var b strings.Builder
	var cm []CellMapping

	for i, w := range colWidths {
		// Opening vertical border (│ is 3 bytes in UTF-8 — one CellMapping per visual cell)
		if i == 0 {
			b.WriteRune('│')
			cm = append(cm, CellMapping{BufOffset: -1})
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

		// Right padding space and closing vertical border (│ is 3 bytes — one CellMapping per visual cell)
		b.WriteByte(' ')
		cm = append(cm, CellMapping{BufOffset: -1})
		b.WriteRune('│')
		cm = append(cm, CellMapping{BufOffset: -1})
	}

	return b.String(), cm
}

// writeRenderedCell writes rendered cell content to the builder using pre-computed cell mapping.
func writeRenderedCell(b *strings.Builder, cm *[]CellMapping, rc renderedCellData) {
	if len(rc.cm) > 0 {
		*cm = append(*cm, rc.cm...)
	} else {
		for pos := 0; pos < len(rc.text); {
			*cm = append(*cm, CellMapping{BufOffset: -1})
			_, size := utf8.DecodeRuneInString(rc.text[pos:])
			if size == 0 {
				size = 1
			}
			pos += size
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

	// Build per-visual-cell kind classification
	type cellInfo struct {
		kind  TokenKind
		marks InlineMarks
		sp    *mdSpan
		cm    CellMapping
	}

	cellInfos := make([]cellInfo, len(cm))

	for i, cellMap := range cm {
		bufOff := cellMap.BufOffset
		kind := TokenTable
		var marks InlineMarks
		var activeSpan *mdSpan

		if bufOff >= 0 {
			relOff := bufOff - lineStart

			// Find the span that covers this offset (the flattened emitter output
			// has exactly one visible span per content byte). Exclude delimiters.
			for _, ms := range sorted {
				contentStart := ms.start + ms.delimLeft
				contentEnd := ms.end - ms.delimRight
				if relOff >= contentStart && relOff < contentEnd {
					kind = ms.kind
					marks = ms.marks
					spanCopy := ms
					activeSpan = &spanCopy
					break
				}
			}
		}

		cellInfos[i] = cellInfo{kind: kind, marks: marks, sp: activeSpan, cm: cellMap}
	}

	// Group consecutive cells with the same kind into spans
	var result []SyntaxSpan
	var currentText strings.Builder
	var currentCM []CellMapping
	var currentKind TokenKind
	var currentMarks InlineMarks
	var currentActiveSpan *mdSpan

	flushSpan := func() {
		if currentText.Len() > 0 {
			sp := SyntaxSpan{
				Text:        currentText.String(),
				Kind:        currentKind,
				Marks:       currentMarks,
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

	// Iterate by runes, tracking CellMap index separately
	runeIdx := 0
	for pos := 0; pos < len(formatted); {
		ci := cellInfos[runeIdx]
		kindChanged := ci.kind != currentKind || ci.marks != currentMarks
		spanChanged := (ci.sp == nil && currentActiveSpan != nil) ||
			(ci.sp != nil && currentActiveSpan == nil) ||
			(ci.sp != nil && currentActiveSpan != nil && ci.sp.kind != currentActiveSpan.kind)

		if kindChanged || spanChanged {
			flushSpan()
			currentKind = ci.kind
			currentMarks = ci.marks
			currentActiveSpan = ci.sp
		}

		r, size := utf8.DecodeRuneInString(formatted[pos:])
		if size == 0 {
			size = 1
			r = utf8.RuneError
		}
		currentText.WriteRune(r)
		currentCM = append(currentCM, ci.cm)
		runeIdx++
		pos += size
	}
	flushSpan()

	return result
}
