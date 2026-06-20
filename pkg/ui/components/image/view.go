package image

import (
	"strings"

	"rune/pkg/imagekit"
)

// View returns the rendered placeholder cells (for Kitty via cell buffer) or
// spaces (for iTerm2 via cell buffer, where placements overlay later).
func (m Model) View() []string {
	if m.cols == 0 || m.rows == 0 || m.visibleRows == 0 || !m.IsLive() || !m.expanded {
		return nil
	}

	lines := make([]string, 0, m.visibleRows)
	var activeID uint32

	if m.termCaps.SupportsKittyGraphics() {
		// active ID depends on animated frame
		activeID = m.id
		if m.animated && m.frameIdx >= 0 && m.frameIdx < len(m.frameIDs) {
			activeID = m.frameIDs[m.frameIdx]
		}
	}

	for i := 0; i < m.visibleRows; i++ {
		rowIdx := m.visibleTop + i
		if rowIdx >= m.rows {
			break
		}

		if m.termCaps.SupportsKittyGraphics() {
			lines = append(lines, imagekit.BuildKittyPlaceholderLine(activeID, rowIdx, m.cols, m.scrollCol, m.maxWidth))
		} else if m.termCaps.SupportsInlineImages() {
			spaces := m.cols - m.scrollCol
			if spaces < 0 {
				spaces = 0
			}
			if spaces > m.maxWidth {
				spaces = m.maxWidth
			}
			lines = append(lines, strings.Repeat(" ", spaces))
		}
	}

	return lines
}

