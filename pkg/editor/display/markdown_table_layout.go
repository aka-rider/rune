package display

import (
	"strings"
)

// ==========================================================================
// Adaptive Layout — Grid, Wrapped Grid, Pivoted Key-Value States
// ==========================================================================

// chooseTableLayout selects the layout kind based on available terminal width.
// Grid is used when the table fits. Wrapped is used when columns can still be
// displayed but need wrapping. Pivoted is used when the terminal is too narrow.
func chooseTableLayout(colWidths []int, availableWidth int) TableLayoutKind {
	if len(colWidths) == 0 {
		return TableLayoutGrid
	}

	// Compute minimum grid width: sum of column content widths + frame overhead
	minGridWidth := computeMinGridWidth(colWidths)

	if minGridWidth <= availableWidth {
		return TableLayoutGrid
	}

	// Compute pivoted width: max(label width + ": " + max value width)
	pivotWidth := computePivotWidth(colWidths)

	// Check if wrapped grid is viable: need at least 2 columns with minimum usable width
	numCols := len(colWidths)
	// Frame overhead: 2 chars per column (│ + padding space) + 1 for final │
	frameWidth := numCols*2 + 1
	usablePerCol := (availableWidth - frameWidth) / numCols

	if usablePerCol >= 4 && availableWidth > pivotWidth {
		return TableLayoutWrapped
	}

	return TableLayoutPivoted
}

// computeMinGridWidth computes the minimum width needed for grid layout.
// This is the sum of column content widths plus frame overhead (│, spaces).
func computeMinGridWidth(colWidths []int) int {
	total := 0
	for _, w := range colWidths {
		total += w // content width
		total += 4 // │ + space + space + │ (but last │ shared with next column)
	}
	// Subtract 1 because the last │ is counted once, not per-column
	total--
	return total
}

// computePivotWidth computes the minimum width for pivoted (key-value) layout.
// This is max(headerTextWidth + ": " + maxValueWidth) across all columns.
func computePivotWidth(colWidths []int) int {
	maxWidth := 0
	for _, w := range colWidths {
		// In pivoted mode: "Header: Value"
		// Header width ≈ column width (header text), ": " = 2, value width
		candidate := w + 2 + w // conservative estimate: header + ": " + value
		if candidate > maxWidth {
			maxWidth = candidate
		}
	}
	return maxWidth
}

// ==========================================================================
// Cell-Aware Wrapping for Wrapped Grid State
// ==========================================================================

// wrapCellSpans wraps a cell's styled spans to fit within maxLineWidth.
// Links (TokenLink, TokenWikiLink, TokenRawURL) are treated as atomic units
// and are not split across lines.
// Returns a slice of wrapped lines, each a slice of SyntaxSpan.
func wrapCellSpans(spans []SyntaxSpan, maxLineWidth int) [][]SyntaxSpan {
	if len(spans) == 0 || maxLineWidth <= 0 {
		return nil
	}

	var result [][]SyntaxSpan
	var currentLine []SyntaxSpan
	currentWidth := 0

	for _, sp := range spans {
		spWidth := runewidthSafe(sp.Text)

		// Check if this span is a link (atomic, non-breakable)
		isLink := sp.Kind == TokenLink || sp.Kind == TokenWikiLink || sp.Kind == TokenRawURL

		if isLink {
			// Links are atomic — if current line has content, flush it
			if currentWidth > 0 {
				result = append(result, currentLine)
				currentLine = nil
				currentWidth = 0
			}
			// Place link on its own line (even if it exceeds maxLineWidth)
			currentLine = append(currentLine, sp)
			currentWidth = spWidth
			continue
		}

		// Regular text — check if it fits on the current line
		if currentWidth+spWidth > maxLineWidth && currentWidth > 0 {
			// Find last space to break at within current line
			breakPos := findLastSpaceBreak(currentLine)
			if breakPos >= 0 {
				// Split at the space
				splitLine, remaining := splitLineAtSpace(currentLine, breakPos)
				result = append(result, splitLine)
				// Start new line with remaining + current span
				currentLine = remaining
				currentWidth = 0
				for _, s := range currentLine {
					currentWidth += runewidthSafe(s.Text)
				}
			} else {
				// No space to break at — flush current line anyway
				result = append(result, currentLine)
				currentLine = nil
				currentWidth = 0
			}
		}

		// Check if adding this span would exceed the line width
		if currentWidth+spWidth > maxLineWidth && currentWidth > 0 {
			result = append(result, currentLine)
			currentLine = nil
			currentWidth = 0
		}

		currentLine = append(currentLine, sp)
		currentWidth += spWidth
	}

	// Flush remaining content
	if len(currentLine) > 0 {
		result = append(result, currentLine)
	}

	// Ensure at least one line
	if len(result) == 0 {
		result = append(result, spans)
	}

	return result
}

// findLastSpaceBreak finds the last span in a line that ends with a space,
// allowing a clean line break. Returns the span index, or -1 if none found.
func findLastSpaceBreak(line []SyntaxSpan) int {
	for i := len(line) - 1; i >= 0; i-- {
		text := line[i].Text
		for j := len(text) - 1; j >= 0; j-- {
			if text[j] == ' ' {
				return i
			}
			break
		}
	}
	return -1
}

// splitLineAtSpace splits a line of spans at the given span index,
// returning the left portion (up to the space) and the right portion (after).
func splitLineAtSpace(line []SyntaxSpan, spanIdx int) (left, right []SyntaxSpan) {
	if spanIdx <= 0 {
		return nil, line
	}
	if spanIdx >= len(line) {
		return line, nil
	}

	left = make([]SyntaxSpan, spanIdx)
	copy(left, line[:spanIdx])
	right = make([]SyntaxSpan, len(line)-spanIdx)
	copy(right, line[spanIdx:])
	return left, right
}

// padEmptySpan creates a space-filler span for padding shorter wrapped cells.
func padEmptySpan(width int) SyntaxSpan {
	text := strings.Repeat(" ", width)
	return SyntaxSpan{
		Text: text,
		Kind: TokenTable,
		State: Rendered,
		CellMap: func() []CellMapping {
			cm := make([]CellMapping, width)
			for i := range cm {
				cm[i] = CellMapping{BufOffset: -1}
			}
			return cm
		}(),
	}
}
