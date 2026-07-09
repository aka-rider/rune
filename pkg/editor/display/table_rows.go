package display

import (
	"strings"
	"unicode/utf8"
)

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

		// Add inter-row separator between body rows
		if isBodyRowBoundary(splitLines, i) {
			addedBorders = true
			border := buildTableBorder(l, separatorBody)
			if border != nil {
				newLines = append(newLines, *border)
			}
		}

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
		if (sp.TableLayout == TableLayoutGrid || sp.TableLayout == TableLayoutWrapped) && sp.BlockID > 0 {
			return sp.BlockID
		}
	}
	return 0
}

// getTableRole returns the table role from a display line's spans.
func getTableRole(l DisplayLine) TableRoleKind {
	for _, sp := range l.Spans {
		if sp.TableRole != 0 {
			return sp.TableRole
		}
	}
	return TableRoleBody
}

// IsTableSeparatorRow reports whether l is a table separator/border display
// row — both genuine "|---|" source rows and the synthetic top/bottom/
// inter-row borders ExpandTableRows inserts carry TableRoleSeparator. Callers
// that skip interactive click targeting on decorative table rows check this
// alongside ImagePath.
func IsTableSeparatorRow(l DisplayLine) bool {
	return getTableRole(l) == TableRoleSeparator
}

// isBodyRowBoundary checks if line at index i is the last display line of a body row
// and the next display line starts a different body row in the same table.
func isBodyRowBoundary(lines []DisplayLine, i int) bool {
	if i >= len(lines)-1 {
		return false
	}
	cur := lines[i]
	next := lines[i+1]

	// Both must be grid/wrapped table lines
	if !isGridTableLine(cur) || !isGridTableLine(next) {
		return false
	}

	// Must be same table block
	if getTableBlockID(cur) != getTableBlockID(next) {
		return false
	}

	// Current must be a body row (not header or separator)
	if getTableRole(cur) != TableRoleBody {
		return false
	}

	// Next must be a body row (not separator or header)
	if getTableRole(next) != TableRoleBody {
		return false
	}

	// They must be different source rows (different ModelLine)
	return cur.ModelLine != next.ModelLine
}

// colWidthsFromLine returns the column widths that formatted l's table row,
// carried on DisplayLine.Spans[*].ColWidths (set uniformly across every span
// a row produces — see formatGridRow/formatWrappedRow/
// formatTableSeparatorSpansWithWidths). Replaces re-parsing the rendered
// │ cell │ cell │ text to recover widths that were already computed once,
// upstream, to build that same text.
func colWidthsFromLine(l DisplayLine) []int {
	for _, sp := range l.Spans {
		if len(sp.ColWidths) > 0 {
			return sp.ColWidths
		}
	}
	return nil
}

// buildTableBorder creates a border DisplayLine for a table.
func buildTableBorder(l DisplayLine, sepType separatorType) *DisplayLine {
	colWidths := colWidthsFromLine(l)
	if len(colWidths) == 0 {
		return nil
	}

	borderText := formatTableSeparatorWithType(colWidths, sepType)
	cm := make([]CellMapping, utf8.RuneCountInString(borderText))
	for i := range cm {
		cm[i] = CellMapping{BufOffset: -1}
	}

	// Get block metadata and layout from source line
	var blockID, blockStart, blockEnd int
	var bufStart, bufEnd int
	layout := TableLayoutGrid
	for _, sp := range l.Spans {
		if sp.BlockID > 0 {
			blockID = sp.BlockID
			blockStart = sp.BlockStart
			blockEnd = sp.BlockEnd
			bufStart = sp.BufferStart
			bufEnd = sp.BufferEnd
			if sp.TableLayout == TableLayoutWrapped {
				layout = TableLayoutWrapped
			}
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
			TableLayout: layout,
		}},
		ModelLine: l.ModelLine,
		WrapRow:   l.WrapRow,
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
				cmOffset++ // skip the \n rune in CellMap
			}
			if len(part) == 0 {
				continue
			}
			var partCM []CellMapping
			partRuneCount := utf8.RuneCountInString(part)
			if sp.CellMap != nil && cmOffset+partRuneCount <= len(sp.CellMap) {
				partCM = sp.CellMap[cmOffset : cmOffset+partRuneCount]
			}
			chunks = append(chunks, textChunk{spanIdx: i, text: part, cm: partCM})
			cmOffset += partRuneCount
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
			ColWidths:      src.ColWidths,
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
				WrapRow:   l.WrapRow,
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
			WrapRow:   l.WrapRow,
		})
	}

	return result
}
