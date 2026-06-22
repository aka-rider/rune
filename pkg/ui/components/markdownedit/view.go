package markdownedit

import (
	"strings"

	"charm.land/lipgloss/v2"

	"rune/pkg/ui/components/textedit"
)

func (m Model) View() string {
	if m.Model.Width() == 0 || m.Model.Height() == 0 {
		return ""
	}

	snap := m.Model.Snapshot()
	vp := m.Model.Viewport()
	contentH := m.Model.ContentHeight()

	lines := snap.Slice(vp.TopRow, contentH)

	cursorStyle := lipgloss.NewStyle().Reverse(true)
	selStyle := m.styles.Selection
	matchStyle := m.styles.SearchMatch
	activeMatchStyle := m.styles.SearchActiveMatch
	cursorOffsets := make(map[int]bool)
	var selections []textedit.SelInterval

	searchMatches := m.Model.SearchMatches()
	searchActive := m.Model.SearchActive()

	focused := m.Model.Focused()
	readOnly := m.Model.ReadOnly()

	if focused && !readOnly {
		for off := range m.Model.CursorOffsets() {
			cursorOffsets[off] = true
		}
		selections = m.Model.Selections()
	} else if readOnly {
		selections = m.Model.Selections()
	}

	imageKitty := m.imageKittyCapable()
	imageInline := m.imageInlineCapable()

	var renderedLines []string
	var imageLineFlags []bool

	for i, l := range lines {
		// Reserved image row: emit Kitty placeholder cells when the image is
		// live (pixels transmitted); otherwise emit blank reserved cells so the
		// layout holds without pointing Kitty at un-transmitted pixel data.
		if l.ImagePath != "" && imageKitty {
			img, live := m.images[l.ImagePath]
			if live {
				live = img.IsLive()
			}
			if !live {
				spaceCells := make([]textedit.Cell, l.ImageCols+1)
				for j := range spaceCells {
					spaceCells[j] = textedit.Cell{Rune: ' ', Width: 1, Style: lipgloss.NewStyle(), BufOffset: -1}
				}
				spaceCells = textedit.SliceCells(spaceCells, vp.ScrollCol, m.Model.Width())
				noStyle := lipgloss.NewStyle()
				renderedLines = append(renderedLines, textedit.CellsToString(spaceCells, selStyle, cursorStyle, noStyle, noStyle))
				imageLineFlags = append(imageLineFlags, true)
				continue
			}
			id := m.imageIDFor(l.ImagePath)
			lineCells := imagePlaceholderCells(id, l.ImageRowIndex, l.ImageCols)
			lineCells = append([]textedit.Cell{{Rune: ' ', Width: 1, Style: lipgloss.NewStyle(), BufOffset: -1}}, lineCells...)
			lineCells = textedit.SliceCells(lineCells, vp.ScrollCol, m.Model.Width())
			noStyle := lipgloss.NewStyle()
			renderedLines = append(renderedLines, textedit.CellsToString(lineCells, selStyle, cursorStyle, noStyle, noStyle))
			imageLineFlags = append(imageLineFlags, true)
			continue
		}

		// iTerm2 inline image row: emit spaces to reserve screen real estate.
		if l.ImagePath != "" && imageInline {
			spaceCells := make([]textedit.Cell, l.ImageCols+1)
			for j := range spaceCells {
				spaceCells[j] = textedit.Cell{Rune: ' ', Width: 1, Style: lipgloss.NewStyle(), BufOffset: -1}
			}
			spaceCells = textedit.SliceCells(spaceCells, vp.ScrollCol, m.Model.Width())
			noStyle := lipgloss.NewStyle()
			renderedLines = append(renderedLines, textedit.CellsToString(spaceCells, selStyle, cursorStyle, noStyle, noStyle))
			imageLineFlags = append(imageLineFlags, true)
			continue
		}

		// Convert spans to styled cells
		var lineCells []textedit.Cell
		for _, sp := range l.Spans {
			lineCells = append(lineCells, m.spanToCellsStyled(sp)...)
		}

		// EOL cursor
		if focused && !readOnly {
			lineEnd := 0
			if len(l.Spans) > 0 {
				last := l.Spans[len(l.Spans)-1]
				lineEnd = last.BufferEnd
			}
			for off := range cursorOffsets {
				if off == lineEnd {
					isLastVisible := i+1 >= len(lines) || lines[i+1].ModelLine != l.ModelLine
					if isLastVisible {
						lineCells = append(lineCells, textedit.Cell{
							Rune:      ' ',
							Width:     1,
							Style:     lipgloss.NewStyle(),
							BufOffset: lineEnd,
						})
					}
					break
				}
			}
		}

		lineCells = textedit.SliceCells(lineCells, vp.ScrollCol, m.Model.Width())

		if focused && (len(cursorOffsets) > 0 || len(selections) > 0) {
			textedit.ApplyOverlays(lineCells, cursorOffsets, selections)
		}

		// Apply match overlay unconditionally (not gated on focused).
		if len(searchMatches) > 0 {
			textedit.ApplyMatchOverlay(lineCells, searchMatches, searchActive)
		}

		renderedLines = append(renderedLines, textedit.CellsToString(lineCells, selStyle, cursorStyle, matchStyle, activeMatchStyle))
		imageLineFlags = append(imageLineFlags, false)
	}

	for len(renderedLines) < contentH {
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

	w := m.Model.Width()
	h := m.Model.Height()

	hasMatches := len(searchMatches) > 0

	var composed string
	if !focused && hasImageLine {
		faint := lipgloss.NewStyle().Faint(true)
		faintedLines := make([]string, len(renderedLines))
		for i, line := range renderedLines {
			if i < len(imageLineFlags) && imageLineFlags[i] {
				faintedLines[i] = line
			} else {
				if hasMatches {
					faintedLines[i] = line
				} else {
					faintedLines[i] = faint.Render(line)
				}
			}
		}
		composed = strings.Join(faintedLines, "\n")
	} else {
		composed = strings.Join(renderedLines, "\n")
		// Suppress faint when search matches are visible so highlights remain legible.
		if !focused && !hasMatches {
			composed = lipgloss.NewStyle().Faint(true).Render(composed)
		}
	}

	return lipgloss.NewStyle().
		MaxWidth(w).
		MaxHeight(h).
		Width(w).
		Height(h).
		Render(composed)
}
