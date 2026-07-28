package markdownedit

import (
	"strings"

	"charm.land/lipgloss/v2"

	"rune/pkg/editor/display"
	"rune/pkg/ui/components/textedit"
)

// View renders through textedit.Model.RenderView (the shared renderer seam,
// §10/CONSTITUTION.md §12): markdownedit supplies its own per-span cell
// builder (spanToCellsStyled — markdown syntax highlighting) and an
// image-row hook closing over its own image/terminal-capability state.
// Everything else (cursor/selection/match overlays, dim-when-unfocused,
// tilde fill, the outer wrap) is textedit's shared pipeline, unchanged.
// blankCells returns n+1 blank (space, unstyled, BufOffset -1) cells — the
// +1 is the left-margin cell every image row reserves. Used to reserve
// screen real estate for a not-yet-live Kitty image and for every iTerm2
// inline image row (which never emits Kitty placeholder cells at all).
func blankCells(n int) []textedit.Cell {
	cells := make([]textedit.Cell, n+1)
	for i := range cells {
		cells[i] = textedit.Cell{Rune: ' ', Width: 1, Style: lipgloss.NewStyle(), BufOffset: -1}
	}
	return cells
}

func (m Model) View() string {
	imageKitty := m.imageKittyCapable()
	imageInline := m.imageInlineCapable()

	imageRow := func(l display.DisplayLine) ([]textedit.Cell, bool) {
		// Reserved image row: emit Kitty placeholder cells when the image is
		// live (pixels transmitted); otherwise emit blank reserved cells so
		// the layout holds without pointing Kitty at un-transmitted pixel
		// data.
		if imageKitty {
			img, live := m.images[l.ImagePath]
			if live {
				live = img.IsLive()
			}
			if !live {
				return blankCells(l.ImageCols), true
			}
			id := m.imageIDFor(l.ImagePath)
			lineCells := imagePlaceholderCells(id, l.ImageRowIndex, l.ImageCols)
			lineCells = append([]textedit.Cell{{Rune: ' ', Width: 1, Style: lipgloss.NewStyle(), BufOffset: -1}}, lineCells...)
			return lineCells, true
		}

		// iTerm2 inline image row: emit spaces to reserve screen real estate.
		if imageInline {
			return blankCells(l.ImageCols), true
		}

		return nil, false
	}

	return m.Model.RenderView(m.spanToCellsStyled, imageRow)
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
