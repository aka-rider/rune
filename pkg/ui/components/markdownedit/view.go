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

	// Dim content when unfocused, except while search matches are shown (keep
	// highlights legible). Applied per style run inside CellsToString.
	dimContent := !focused && len(searchMatches) == 0

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
				renderedLines = append(renderedLines, textedit.CellsToString(spaceCells, selStyle, cursorStyle, noStyle, noStyle, false))
				continue
			}
			id := m.imageIDFor(l.ImagePath)
			lineCells := imagePlaceholderCells(id, l.ImageRowIndex, l.ImageCols)
			lineCells = append([]textedit.Cell{{Rune: ' ', Width: 1, Style: lipgloss.NewStyle(), BufOffset: -1}}, lineCells...)
			lineCells = textedit.SliceCells(lineCells, vp.ScrollCol, m.Model.Width())
			noStyle := lipgloss.NewStyle()
			renderedLines = append(renderedLines, textedit.CellsToString(lineCells, selStyle, cursorStyle, noStyle, noStyle, false))
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
			renderedLines = append(renderedLines, textedit.CellsToString(spaceCells, selStyle, cursorStyle, noStyle, noStyle, false))
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

		renderedLines = append(renderedLines, textedit.CellsToString(lineCells, selStyle, cursorStyle, matchStyle, activeMatchStyle, dimContent))
	}

	// Filler tildes carry no embedded ANSI, so a single faint render is correct.
	tilde := "~"
	if dimContent {
		tilde = lipgloss.NewStyle().Faint(true).Render("~")
	}
	for len(renderedLines) < contentH {
		renderedLines = append(renderedLines, tilde)
	}

	w := m.Model.Width()
	h := m.Model.Height()

	// Dimming is applied per style run inside CellsToString (dimContent), so the
	// composed string is assembled as-is — no post-hoc Faint wrap (which would be
	// cleared by the embedded resets of inner styled runs, e.g. links).
	composed := strings.Join(renderedLines, "\n")

	return lipgloss.NewStyle().
		MaxWidth(w).
		MaxHeight(h).
		Width(w).
		Height(h).
		Render(composed)
}

// RenderEmpty renders the editor's empty frame (the Vim-style "~" fill) at the
// current dimensions WITHOUT touching the buffer — pure, no I/O, no mutation
// (§5.2). The workspace substitutes it for View() while a file load is in flight
// so the center pane blanks without the old destructive SetContent("") that
// stranded the editor on a failed load. It reproduces View()'s empty-buffer
// output exactly (same ContentHeight tildes, same faint-when-unfocused, same
// outer wrap) so the pane height never jumps between the pending and loaded frame.
func (m Model) RenderEmpty() string {
	w := m.Model.Width()
	h := m.Model.Height()
	if w == 0 || h == 0 {
		return ""
	}
	lines := make([]string, m.Model.ContentHeight())
	for i := range lines {
		lines[i] = "~"
	}
	composed := strings.Join(lines, "\n")
	if !m.Model.Focused() {
		composed = lipgloss.NewStyle().Faint(true).Render(composed)
	}
	return lipgloss.NewStyle().
		MaxWidth(w).
		MaxHeight(h).
		Width(w).
		Height(h).
		Render(composed)
}
