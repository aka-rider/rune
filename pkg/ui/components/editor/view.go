package editor

import (
	"strings"

	"charm.land/lipgloss/v2"
)

func (m Model) View() string {
	if m.width == 0 || m.height == 0 {
		return ""
	}

	titleView := m.title.View()
	contentHeight := m.contentHeight()

	// Vertical slice only — horizontal scrolling is done at cell level
	lines := m.snapshot.Slice(m.viewport.TopRow, contentHeight)

	// Collect cursor byte offsets and selection intervals for rendering.
	cursorStyle := lipgloss.NewStyle().Reverse(true)
	selStyle := m.styles.Selection
	cursorOffsets := make(map[int]bool)
	var selections []selInterval
	if m.focused {
		for _, c := range m.cursors.All() {
			cursorOffsets[c.Position] = true
			if c.HasSelection() {
				selections = append(selections, selInterval{c.SelectionStart(), c.SelectionEnd()})
			}
		}
	}

	imageCapable := m.imageKittyCapable()
	inlineCapable := m.imageInlineCapable()

	var renderedLines []string
	var imageLineFlags []bool
	for _, l := range lines {
		// Reserved image row: emit Kitty placeholder cells instead of span
		// cells. The cells flow through sliceCells like any other content, so
		// horizontal scroll/clip is handled uniformly.
		if l.ImagePath != "" && imageCapable {
			id := m.imageIDFor(l.ImagePath)
			lineCells := imagePlaceholderCells(id, l.ImageRowIndex, l.ImageCols)
			// Prepend 1-cell left margin so image is not flush against the border.
			lineCells = append([]Cell{{Rune: ' ', Width: 1, Style: lipgloss.NewStyle(), BufOffset: -1}}, lineCells...)
			lineCells = sliceCells(lineCells, m.viewport.ScrollCol, m.width)
			renderedLines = append(renderedLines, cellsToString(lineCells, selStyle, cursorStyle))
			imageLineFlags = append(imageLineFlags, true)
			continue
		}

		// iTerm2 inline image row: emit spaces to reserve screen real estate.
		// The actual image is painted separately via tea.Raw (emitInlinePlacements).
		if l.ImagePath != "" && inlineCapable {
			spaceCells := make([]Cell, l.ImageCols+1) // +1 for left margin
			for i := range spaceCells {
				spaceCells[i] = Cell{Rune: ' ', Width: 1, Style: lipgloss.NewStyle(), BufOffset: -1}
			}
			spaceCells = sliceCells(spaceCells, m.viewport.ScrollCol, m.width)
			renderedLines = append(renderedLines, cellsToString(spaceCells, selStyle, cursorStyle))
			imageLineFlags = append(imageLineFlags, true)
			continue
		}

		// Convert all spans to cells
		var lineCells []Cell
		for _, sp := range l.Spans {
			spCells := m.spanToCellsStyled(sp)
			lineCells = append(lineCells, spCells...)
		}

		// EOL cursor: append synthetic cell if cursor is at end-of-line
		if m.focused {
			lineEnd := 0
			if len(l.Spans) > 0 {
				last := l.Spans[len(l.Spans)-1]
				lineEnd = last.BufferEnd
			}
			for off := range cursorOffsets {
				if off == lineEnd {
					lineCells = append(lineCells, Cell{
						Rune:      ' ',
						Width:     1,
						Style:     lipgloss.NewStyle(),
						BufOffset: lineEnd,
					})
					break
				}
			}
		}

		// Horizontal scrolling at cell level
		lineCells = sliceCells(lineCells, m.viewport.ScrollCol, m.width)

		// Apply cursor and selection overlays
		if m.focused && (len(cursorOffsets) > 0 || len(selections) > 0) {
			applyOverlays(lineCells, cursorOffsets, selections)
		}

		// Stringify
		renderedLines = append(renderedLines, cellsToString(lineCells, selStyle, cursorStyle))
		imageLineFlags = append(imageLineFlags, false)
	}

	for len(renderedLines) < contentHeight {
		renderedLines = append(renderedLines, "~")
		imageLineFlags = append(imageLineFlags, false)
	}

	hasImageLine := false
	for _, f := range imageLineFlags {
		if f {
			hasImageLine = true
			break
		}
	}

	var composed string
	if !m.focused && hasImageLine {
		// Faint per line, leaving image lines untouched: the placeholder
		// foreground color carries the image ID and must never be dimmed.
		faint := lipgloss.NewStyle().Faint(true)
		faintedLines := make([]string, len(renderedLines))
		for i, line := range renderedLines {
			if i < len(imageLineFlags) && imageLineFlags[i] {
				faintedLines[i] = line
			} else {
				faintedLines[i] = faint.Render(line)
			}
		}
		content := strings.Join(faintedLines, "\n")
		composed = lipgloss.JoinVertical(lipgloss.Left, faint.Render(titleView), content)
	} else {
		content := strings.Join(renderedLines, "\n")
		composed = lipgloss.JoinVertical(lipgloss.Left, titleView, content)
		if !m.focused {
			composed = lipgloss.NewStyle().Faint(true).Render(composed)
		}
	}

	return lipgloss.NewStyle().
		MaxWidth(m.width).
		MaxHeight(m.height).
		Width(m.width).
		Height(m.height).
		Render(composed)
}

// InlineImagePlacements returns the escape sequences for iTerm2/WezTerm inline
// image placement. The editor emits these via tea.Raw from emitInlinePlacements
// (Update), bypassing the cell renderer; this accessor is retained for tests
// that inspect the computed sequence.
func (m Model) InlineImagePlacements() string {
	return m.buildInlineImagePlacements()
}
