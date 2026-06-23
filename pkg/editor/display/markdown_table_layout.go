package display

import (
	"strings"
)

// ==========================================================================
// Adaptive Layout — Grid, Wrapped Grid, Pivoted Key-Value States
// ==========================================================================

// chooseTableLayout selects the layout kind based on available terminal width.
// Grid is used when the table fits as-is. Wrapped (constrained grid) is used
// when the table can be shrunk proportionally and still be readable. Pivoted
// is used when even a constrained grid would be unreadable.
func chooseTableLayout(colWidths []int, minColWidths []int, availableWidth int) TableLayoutKind {
	if len(colWidths) == 0 {
		return TableLayoutGrid
	}

	minGridWidth := computeMinGridWidth(colWidths)
	if minGridWidth <= availableWidth {
		return TableLayoutGrid
	}

	// Check if a constrained grid is viable. We need enough space for:
	// - Atomic columns (URLs, long words) at their minimum width
	// - Flexible columns at minimum 12 chars each for readability
	numCols := len(colWidths)
	frameOverhead := 4*numCols - 1
	contentBudget := availableWidth - frameOverhead

	if contentBudget <= 0 {
		return TableLayoutPivoted
	}

	// Compute how much space atomic columns require
	minFlexWidth := 12
	atomicBudget := 0
	flexCount := 0

	for i := range colWidths {
		minW := 0
		if i < len(minColWidths) {
			minW = minColWidths[i]
		}
		// A column is "atomic-dominant" if its longest word exceeds a comfortable
		// share of the budget (i.e., it can't reasonably be shrunk)
		equalShare := contentBudget / numCols
		if minW > equalShare {
			atomicBudget += minW
		} else {
			flexCount++
		}
	}

	// Check if flexible columns get enough space
	flexBudget := contentBudget - atomicBudget
	if flexCount > 0 && flexBudget >= flexCount*minFlexWidth {
		return TableLayoutWrapped
	}

	// Even without atomic columns, check basic viability
	if flexCount == 0 && atomicBudget <= contentBudget {
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

// constrainColWidths shrinks column widths so the grid fits within availableWidth.
// Algorithm: give each column its minimum width (longest unbreakable word),
// then distribute remaining space proportionally to how much each column
// wants beyond its minimum.
func constrainColWidths(colWidths []int, minColWidths []int, availableWidth int) []int {
	if len(colWidths) == 0 {
		return colWidths
	}

	minGridWidth := computeMinGridWidth(colWidths)
	if minGridWidth <= availableWidth {
		return colWidths
	}

	numCols := len(colWidths)
	frameOverhead := 4*numCols - 1
	contentBudget := availableWidth - frameOverhead
	if contentBudget < numCols {
		contentBudget = numCols
	}

	result := make([]int, numCols)

	// Step 1: Give each column its minimum width (longest unbreakable word)
	floorTotal := 0
	for i := range colWidths {
		floor := 3
		if i < len(minColWidths) && minColWidths[i] > floor {
			floor = minColWidths[i]
		}
		// Don't exceed natural width
		if floor > colWidths[i] {
			floor = colWidths[i]
		}
		result[i] = floor
		floorTotal += floor
	}

	// Step 2: Distribute remaining budget proportionally to "stretch" demand
	remaining := contentBudget - floorTotal
	if remaining <= 0 {
		return result
	}

	// Each column's stretch demand is how much more it wants beyond its floor
	totalStretch := 0
	for i, w := range colWidths {
		stretch := w - result[i]
		if stretch > 0 {
			totalStretch += stretch
		}
		_ = stretch
	}

	if totalStretch == 0 {
		// All columns already at natural width — distribute equally
		perCol := remaining / numCols
		for i := range result {
			result[i] += perCol
		}
		return result
	}

	// Distribute proportionally to stretch demand
	leftover := remaining
	for i, w := range colWidths {
		stretch := w - result[i]
		if stretch <= 0 {
			continue
		}
		alloc := (stretch * remaining) / totalStretch
		if alloc > stretch {
			alloc = stretch // never exceed natural width
		}
		result[i] += alloc
		leftover -= alloc
	}

	// Give any rounding remainder to the widest column
	if leftover > 0 {
		widest := 0
		for i := 1; i < numCols; i++ {
			if colWidths[i] > colWidths[widest] {
				widest = i
			}
		}
		result[widest] += leftover
	}

	return result
}

// wrapCellText wraps plain text to fit within maxWidth visual cells.
// Words are broken at space boundaries. URLs (tokens starting with http:// or
// https://) are treated as atomic and never broken mid-token.
func wrapCellText(text string, maxWidth int) []string {
	if maxWidth <= 0 {
		return []string{text}
	}
	text = strings.TrimSpace(text)
	if runewidthSafe(text) <= maxWidth {
		return []string{text}
	}

	words := strings.Fields(text)
	if len(words) == 0 {
		return []string{""}
	}

	var lines []string
	var currentLine strings.Builder
	currentWidth := 0

	for _, word := range words {
		wordWidth := runewidthSafe(word)

		// If this is the first word on the line, always place it (even if it exceeds maxWidth)
		if currentWidth == 0 {
			// If a single word exceeds maxWidth and it's NOT a URL, hard-break it
			if wordWidth > maxWidth && !isURL(word) {
				lines = append(lines, hardBreakWord(word, maxWidth, &currentLine, &currentWidth)...)
			} else {
				currentLine.WriteString(word)
				currentWidth = wordWidth
			}
			continue
		}

		// Check if word fits on current line (with space separator)
		if currentWidth+1+wordWidth <= maxWidth {
			currentLine.WriteByte(' ')
			currentLine.WriteString(word)
			currentWidth += 1 + wordWidth
		} else {
			// Flush current line, start a new one
			lines = append(lines, currentLine.String())
			currentLine.Reset()
			// Place word on new line (even if it exceeds — URLs are atomic)
			if wordWidth > maxWidth && !isURL(word) {
				currentWidth = 0
				lines = append(lines, hardBreakWord(word, maxWidth, &currentLine, &currentWidth)...)
			} else {
				currentLine.WriteString(word)
				currentWidth = wordWidth
			}
		}
	}

	// Flush remaining
	if currentLine.Len() > 0 {
		lines = append(lines, currentLine.String())
	}

	if len(lines) == 0 {
		return []string{""}
	}
	return lines
}

// isURL returns true if the word looks like a URL.
func isURL(word string) bool {
	return strings.HasPrefix(word, "http://") || strings.HasPrefix(word, "https://")
}

// hardBreakWord breaks a long non-URL word into chunks of maxWidth characters.
// Returns completed lines and leaves any remainder in currentLine/currentWidth.
func hardBreakWord(word string, maxWidth int, currentLine *strings.Builder, currentWidth *int) []string {
	var lines []string
	runes := []rune(word)
	for len(runes) > 0 {
		take := maxWidth
		if take > len(runes) {
			take = len(runes)
		}
		chunk := string(runes[:take])
		runes = runes[take:]
		if len(runes) > 0 {
			// More chunks follow — this line is complete
			lines = append(lines, chunk)
		} else {
			// Last chunk — leave it in currentLine
			currentLine.Reset()
			currentLine.WriteString(chunk)
			*currentWidth = runewidthSafe(chunk)
		}
	}
	return lines
}
