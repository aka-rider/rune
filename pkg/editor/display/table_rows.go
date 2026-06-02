package display

import "strings"

// ExpandTableRows returns a new snapshot in which:
//  1. Any DisplayLine containing newline characters in its span text is split
//     into multiple display rows (for pivot mode).
//  2. Grid-mode tables get top (┌──┬──┐) and bottom (└──┴──┘) borders.
//
// Each generated row shares the same ModelLine as the original.
func ExpandTableRows(ds DisplaySnapshot) DisplaySnapshot {
	// Pass 1: Split lines containing \n into multiple display rows
	splitLines := make([]DisplayLine, 0, len(ds.Lines))
	didSplit := false

	for _, l := range ds.Lines {
		if lineNeedsSplit(l) {
			didSplit = true
			subLines := splitDisplayLine(l)
			splitLines = append(splitLines, subLines...)
		} else {
			splitLines = append(splitLines, l)
		}
	}

	// Pass 2: Add top/bottom borders for grid-layout tables
	newLines := make([]DisplayLine, 0, len(splitLines))
	addedBorders := false

	for i, l := range splitLines {
		if isGridTableLine(l) {
			if isFirstTableLine(splitLines, i) {
				addedBorders = true
				border := buildTableBorder(l, separatorTop)
				if border != nil {
					newLines = append(newLines, *border)
				}
			}
		}

		newLines = append(newLines, l)

		if isGridTableLine(l) && isLastTableLine(splitLines, i) {
			addedBorders = true
			border := buildTableBorder(l, separatorBottom)
			if border != nil {
				newLines = append(newLines, *border)
			}
		}
	}

	if !didSplit && !addedBorders {
		return ds
	}

	rowToModelLine := make([]int, len(newLines))
	lineToFirstRow := make([]int, len(ds.lineToFirstRow))
	seen := make([]bool, len(ds.lineToFirstRow))
	for r, nl := range newLines {
		rowToModelLine[r] = nl.ModelLine
		if nl.ModelLine >= 0 && nl.ModelLine < len(lineToFirstRow) && !seen[nl.ModelLine] {
			lineToFirstRow[nl.ModelLine] = r
			seen[nl.ModelLine] = true
		}
	}

	return DisplaySnapshot{
		Lines:          newLines,
		TotalRows:      len(newLines),
		rowToModelLine: rowToModelLine,
		lineToFirstRow: lineToFirstRow,
	}
}

// isGridTableLine checks if a display line is part of a grid-layout table
// (either natural grid or constrained/wrapped grid).
func isGridTableLine(l DisplayLine) bool {
	for _, sp := range l.Spans {
		if (sp.TableLayout == TableLayoutGrid || sp.TableLayout == TableLayoutWrapped) && sp.BlockID > 0 {
			return true
		}
	}
	return false
}

// isFirstTableLine checks if line at index i is the first line of its table block.
func isFirstTableLine(lines []DisplayLine, i int) bool {
	if i == 0 {
		return true
	}
	blockID := getTableBlockID(lines[i])
	if blockID <= 0 {
		return false
	}
	prevBlockID := getTableBlockID(lines[i-1])
	return prevBlockID != blockID
}

// isLastTableLine checks if line at index i is the last line of its table block.
func isLastTableLine(lines []DisplayLine, i int) bool {
	if i >= len(lines)-1 {
		return true
	}
	blockID := getTableBlockID(lines[i])
	if blockID <= 0 {
		return false
	}
	nextBlockID := getTableBlockID(lines[i+1])
	return nextBlockID != blockID
}

// getTableBlockID returns the table block ID from a display line's spans.
func getTableBlockID(l DisplayLine) int {
	for _, sp := range l.Spans {
		if sp.TableLayout == TableLayoutGrid && sp.BlockID > 0 {
			return sp.BlockID
		}
	}
	return 0
}

// getTableColWidths extracts column widths by parsing the rendered row text.
// Grid rows have format: │ cell │ cell │ — we count the content width between pipes.
func getTableColWidths(l DisplayLine) []int {
	text := ""
	for _, sp := range l.Spans {
		text += sp.Text
	}
	if len(text) == 0 {
		return nil
	}

	// Parse column widths from the rendered row format: │ content │ content │
	// Each column contributes: │ + space + content + space (the last has trailing │)
	var widths []int
	inCell := false
	cellWidth := 0

	i := 0
	for i < len(text) {
		// Check for │ (3 bytes: E2 94 82)
		if i+2 < len(text) && text[i] == 0xE2 && text[i+1] == 0x94 && text[i+2] == 0x82 {
			if inCell {
				// End of cell — subtract the padding spaces (1 before content + 1 after)
				// The cellWidth includes the leading space, content, and trailing space
				w := cellWidth - 2 // subtract leading and trailing space
				if w < 0 {
					w = 0
				}
				widths = append(widths, w)
				cellWidth = 0
			}
			inCell = true
			i += 3
			cellWidth = 0
			continue
		}

		if inCell {
			cellWidth++
		}
		i++
	}

	return widths
}

// buildTableBorder creates a border DisplayLine for a table.
func buildTableBorder(l DisplayLine, sepType separatorType) *DisplayLine {
	colWidths := getTableColWidths(l)
	if len(colWidths) == 0 {
		return nil
	}

	borderText := formatTableSeparatorWithType(colWidths, sepType)
	cm := make([]CellMapping, len(borderText))
	for i := range cm {
		cm[i] = CellMapping{BufOffset: -1}
	}

	// Get block metadata from source line
	var blockID, blockStart, blockEnd int
	var bufStart, bufEnd int
	for _, sp := range l.Spans {
		if sp.BlockID > 0 {
			blockID = sp.BlockID
			blockStart = sp.BlockStart
			blockEnd = sp.BlockEnd
			bufStart = sp.BufferStart
			bufEnd = sp.BufferEnd
			break
		}
	}

	return &DisplayLine{
		Spans: []DisplaySpan{{
			Text:        borderText,
			Kind:        TokenTable,
			State:       Rendered,
			BufferStart: bufStart,
			BufferEnd:   bufEnd,
			CellMap:     cm,
			BlockID:     blockID,
			BlockStart:  blockStart,
			BlockEnd:    blockEnd,
			TableRole:   TableRoleSeparator,
			TableLayout: TableLayoutGrid,
		}},
		ModelLine: l.ModelLine,
		WrapIndex: 0,
	}
}

// lineNeedsSplit checks if any span in the line contains a newline character.
func lineNeedsSplit(l DisplayLine) bool {
	for _, sp := range l.Spans {
		if strings.ContainsRune(sp.Text, '\n') {
			return true
		}
	}
	return false
}

// splitDisplayLine splits a DisplayLine at newline boundaries within spans,
// producing one DisplayLine per visual row.
func splitDisplayLine(l DisplayLine) []DisplayLine {
	// Split each span at \n boundaries into chunks
	type textChunk struct {
		spanIdx   int
		text      string
		cm        []CellMapping
		isNewline bool
	}

	var chunks []textChunk
	for i, sp := range l.Spans {
		parts := strings.Split(sp.Text, "\n")
		cmOffset := 0
		for pi, part := range parts {
			if pi > 0 {
				chunks = append(chunks, textChunk{spanIdx: i, isNewline: true})
				cmOffset++ // skip the \n byte in CellMap
			}
			if len(part) == 0 {
				continue
			}
			var partCM []CellMapping
			if sp.CellMap != nil && cmOffset+len(part) <= len(sp.CellMap) {
				partCM = sp.CellMap[cmOffset : cmOffset+len(part)]
			}
			chunks = append(chunks, textChunk{spanIdx: i, text: part, cm: partCM})
			cmOffset += len(part)
		}
	}

	// Group chunks into lines (split at newline markers)
	var result []DisplayLine
	var currentSpans []DisplaySpan
	var currentText strings.Builder
	var currentCM []CellMapping
	lastSpanIdx := -1

	flushSpan := func(spanIdx int) {
		if currentText.Len() == 0 && len(currentCM) == 0 {
			return
		}
		src := l.Spans[spanIdx]
		currentSpans = append(currentSpans, DisplaySpan{
			Text:           currentText.String(),
			Kind:           src.Kind,
			State:          src.State,
			BufferStart:    src.BufferStart,
			BufferEnd:      src.BufferEnd,
			CellMap:        currentCM,
			Language:       src.Language,
			BlockID:        src.BlockID,
			BlockStart:     src.BlockStart,
			BlockEnd:       src.BlockEnd,
			TableRole:      src.TableRole,
			TableLayout:    src.TableLayout,
			WikiLinkTarget: src.WikiLinkTarget,
			LinkURL:        src.LinkURL,
		})
		currentText.Reset()
		currentCM = nil
	}

	flushLine := func() {
		if lastSpanIdx >= 0 {
			flushSpan(lastSpanIdx)
		}
		// Always produce a line (even empty ones are valid for spacing)
		if len(currentSpans) > 0 {
			result = append(result, DisplayLine{
				Spans:     currentSpans,
				ModelLine: l.ModelLine,
				WrapIndex: len(result),
			})
		}
		currentSpans = nil
	}

	for _, ch := range chunks {
		if ch.isNewline {
			if lastSpanIdx >= 0 {
				flushSpan(lastSpanIdx)
			}
			flushLine()
			lastSpanIdx = -1
			continue
		}

		// If span changed, flush previous
		if lastSpanIdx >= 0 && lastSpanIdx != ch.spanIdx {
			flushSpan(lastSpanIdx)
		}
		lastSpanIdx = ch.spanIdx
		currentText.WriteString(ch.text)
		currentCM = append(currentCM, ch.cm...)
	}

	// Final flush
	if lastSpanIdx >= 0 {
		flushSpan(lastSpanIdx)
	}
	flushLine()

	// If nothing was produced (all empty), produce one empty line
	if len(result) == 0 {
		result = append(result, DisplayLine{
			Spans:     nil,
			ModelLine: l.ModelLine,
			WrapIndex: 0,
		})
	}

	return result
}
