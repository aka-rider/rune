package textedit

import (
	"strings"
	"unicode/utf8"

	"charm.land/lipgloss/v2"
	"github.com/mattn/go-runewidth"

	"rune/pkg/editor/display"
)

// Cell represents a single visual character in the cell grid.
type Cell struct {
	Rune      rune
	Grapheme  string
	Width     int
	Style     lipgloss.Style
	BufOffset int
	Selected  bool
	Cursor    bool
}

// SelInterval is a selection range [Start, End) used for overlay rendering.
type SelInterval struct{ Start, End int }

// SpanToCells converts a DisplaySpan into a slice of Cells.
func SpanToCells(sp display.DisplaySpan, baseStyle lipgloss.Style) []Cell {
	if sp.State == display.Revealed {
		return revealedSpanToCells(sp.Text, sp.BufferStart, baseStyle)
	}
	if sp.CellMap == nil {
		return revealedSpanToCells(sp.Text, sp.BufferStart, baseStyle)
	}
	return renderedSpanToCells(sp.Text, sp.CellMap, baseStyle)
}

// revealedSpanToCells creates cells for text where bytes map 1:1 to buffer offsets.
func revealedSpanToCells(text string, bufStart int, style lipgloss.Style) []Cell {
	cells := make([]Cell, 0, len(text))
	pos := 0
	for pos < len(text) {
		r, size := utf8.DecodeRuneInString(text[pos:])
		if size == 0 {
			size = 1
			r = utf8.RuneError
		}
		if r == '\n' || r == '\r' {
			pos += size
			continue
		}
		w := runewidth.RuneWidth(r)
		if w == 0 {
			w = 1
		}
		cells = append(cells, Cell{
			Rune:      r,
			Width:     w,
			Style:     style,
			BufOffset: bufStart + pos,
		})
		pos += size
	}
	return cells
}

// renderedSpanToCells creates cells for rendered text using the provided CellMap.
func renderedSpanToCells(text string, cm []display.CellMapping, style lipgloss.Style) []Cell {
	cells := make([]Cell, 0, utf8.RuneCountInString(text))
	pos := 0
	mapIdx := 0
	for pos < len(text) {
		r, size := utf8.DecodeRuneInString(text[pos:])
		if size == 0 {
			size = 1
			r = utf8.RuneError
		}
		if r == '\n' || r == '\r' {
			pos += size
			mapIdx++
			continue
		}
		w := runewidth.RuneWidth(r)
		if w == 0 {
			w = 1
		}
		bufOff := -1
		if mapIdx < len(cm) {
			bufOff = cm[mapIdx].BufOffset
		}
		cells = append(cells, Cell{
			Rune:      r,
			Width:     w,
			Style:     style,
			BufOffset: bufOff,
		})
		mapIdx++
		pos += size
	}
	return cells
}

// isInSelection reports whether a byte offset falls within any selection interval.
func isInSelection(off int, selections []SelInterval) bool {
	for _, sel := range selections {
		if off >= sel.Start && off < sel.End {
			return true
		}
	}
	return false
}

// ApplyOverlays marks cells as Selected or Cursor based on their BufOffset.
func ApplyOverlays(cells []Cell, cursorOffsets map[int]bool, selections []SelInterval) {
	for i := range cells {
		if cells[i].BufOffset < 0 {
			continue
		}
		if cursorOffsets[cells[i].BufOffset] {
			cells[i].Cursor = true
		}
		if isInSelection(cells[i].BufOffset, selections) {
			cells[i].Selected = true
		}
	}
}

// SliceCells performs horizontal scrolling at the cell level.
func SliceCells(cells []Cell, scrollCol, viewWidth int) []Cell {
	if scrollCol <= 0 && viewWidth <= 0 {
		return cells
	}
	if viewWidth <= 0 {
		viewWidth = 80
	}

	result := make([]Cell, 0, viewWidth)
	col := 0

	i := 0
	for i < len(cells) {
		if col+cells[i].Width > scrollCol {
			if col < scrollCol {
				padWidth := col + cells[i].Width - scrollCol
				for p := 0; p < padWidth; p++ {
					result = append(result, Cell{
						Rune:      ' ',
						Width:     1,
						Style:     cells[i].Style,
						BufOffset: -1,
					})
				}
				col = scrollCol + padWidth
				i++
			}
			break
		}
		col += cells[i].Width
		i++
	}

	usedWidth := col - scrollCol
	for i < len(cells) && usedWidth < viewWidth {
		c := cells[i]
		if usedWidth+c.Width > viewWidth {
			break
		}
		result = append(result, c)
		usedWidth += c.Width
		i++
	}

	return result
}

// CellsToString converts a slice of cells to a final ANSI-styled string.
func CellsToString(cells []Cell, selStyle, cursorStyle lipgloss.Style) string {
	if len(cells) == 0 {
		return ""
	}

	var b strings.Builder
	b.Grow(len(cells) * 2)

	i := 0
	for i < len(cells) {
		effectiveStyle := cellEffectiveStyle(cells[i], selStyle, cursorStyle)
		j := i + 1
		for j < len(cells) {
			nextStyle := cellEffectiveStyle(cells[j], selStyle, cursorStyle)
			if !stylesEqual(effectiveStyle, nextStyle) {
				break
			}
			j++
		}

		var run strings.Builder
		for k := i; k < j; k++ {
			if cells[k].Grapheme != "" {
				run.WriteString(cells[k].Grapheme)
				continue
			}
			run.WriteRune(cells[k].Rune)
		}

		b.WriteString(effectiveStyle.Render(run.String()))
		i = j
	}

	return b.String()
}

// cellEffectiveStyle computes the final style for a cell considering overlays.
func cellEffectiveStyle(c Cell, selStyle, cursorStyle lipgloss.Style) lipgloss.Style {
	if c.Cursor {
		return cursorStyle
	}
	if c.Selected {
		return c.Style.Background(selStyle.GetBackground())
	}
	return c.Style
}

// stylesEqual compares two lipgloss styles for equality.
func stylesEqual(a, b lipgloss.Style) bool {
	return a.Render(" ") == b.Render(" ")
}
