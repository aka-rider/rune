package imagekit

import (
	"fmt"
	"strings"
)

// BuildKittyPlaceholderLine returns the escape sequence for one row of Kitty
// Unicode virtual-placement cells. id is encoded as a 24-bit truecolor
// foreground; rowIdx and each column index become combining diacritics on
// U+10EEEE (Placeholder). scrollCol / maxWidth clip the output for horizontal
// scrolling — pass scrollCol=0 and maxWidth=cols for a full unscrolled line.
func BuildKittyPlaceholderLine(id uint32, rowIdx, cols, scrollCol, maxWidth int) string {
	r := (id >> 16) & 0xFF
	g := (id >> 8) & 0xFF
	b := id & 0xFF

	start := scrollCol
	if start < 0 {
		start = 0
	}
	end := start + maxWidth
	if end > cols {
		end = cols
	}
	if start >= end {
		return ""
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("\x1b[38;2;%d;%d;%dm", r, g, b))
	for col := start; col < end; col++ {
		sb.WriteRune(Placeholder)
		sb.WriteRune(Diacritic(rowIdx))
		sb.WriteRune(Diacritic(col))
	}
	sb.WriteString("\x1b[0m")
	return sb.String()
}
