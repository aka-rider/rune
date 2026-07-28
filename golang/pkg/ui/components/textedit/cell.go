package textedit

import (
	"image/color"
	"strings"
	"unicode/utf8"

	"charm.land/lipgloss/v2"

	"rune/pkg/editor/display"
)

// Cell represents a single visual character in the cell grid.
type Cell struct {
	Rune        rune
	Grapheme    string
	Width       int
	Style       lipgloss.Style
	BufOffset   int
	Selected    bool
	Cursor      bool
	Match       bool // part of a search match
	ActiveMatch bool // part of the currently-active search match
}

// SelInterval is a selection range [Start, End) used for overlay rendering.
type SelInterval struct{ Start, End int }

// BgInterval tints the background of every cell whose BufOffset ∈ [Start,End)
// with Color, as part of the base cell style (cursor/selection/match overlays
// still take precedence via cellEffectiveStyle). A render-time overlay — it
// reads from caller-supplied ranges, never from buffer text, so it stays
// merge-agnostic and reusable by any textedit consumer.
type BgInterval struct {
	Start, End int
	Color      color.Color
}

// ApplyBackgroundIntervals sets a background colour on every cell whose
// BufOffset falls within one of ivs. Called BEFORE sliceCells (in the
// cell-builder, not View) so the tint is part of the base cell style; cursor /
// selection / match overlays applied later still take precedence.
func ApplyBackgroundIntervals(cells []Cell, ivs []BgInterval) {
	for i := range cells {
		off := cells[i].BufOffset
		if off < 0 {
			continue
		}
		for _, iv := range ivs {
			if off >= iv.Start && off < iv.End {
				cells[i].Style = cells[i].Style.Background(iv.Color)
				break
			}
		}
	}
}

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
		w := display.ControlAwareWidth(r)
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
		w := display.ControlAwareWidth(r)
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

// applyOverlays marks cells as Selected or Cursor based on their BufOffset.
// D5: unexported — no caller outside this package.
func applyOverlays(cells []Cell, cursorOffsets map[int]bool, selections []SelInterval) {
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

// sliceCells performs horizontal scrolling at the cell level. D5: unexported
// — no caller outside this package.
func sliceCells(cells []Cell, scrollCol, viewWidth int) []Cell {
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

// cellsToString converts a slice of cells to a final ANSI-styled string.
// matchStyle and activeMatchStyle are used for search-highlight overlays;
// pass lipgloss.NewStyle() when no search is active.
//
// dim folds Faint(true) into every style run (used when the editor is unfocused).
// It MUST be applied per-run here rather than wrapping the assembled string: the
// embedded \x1b[0m reset at the end of each styled run would otherwise clear the
// faint for the remainder of the line (only the first run would dim).
//
// D5: unexported — no caller outside this package.
func cellsToString(cells []Cell, selStyle, cursorStyle, matchStyle, activeMatchStyle lipgloss.Style, dim bool) string {
	if len(cells) == 0 {
		return ""
	}

	var b strings.Builder
	b.Grow(len(cells) * 2)

	i := 0
	for i < len(cells) {
		effectiveStyle := cellEffectiveStyle(cells[i], selStyle, cursorStyle, matchStyle, activeMatchStyle, dim)
		j := i + 1
		for j < len(cells) {
			nextStyle := cellEffectiveStyle(cells[j], selStyle, cursorStyle, matchStyle, activeMatchStyle, dim)
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

// applyMatchOverlay marks cells as Match or ActiveMatch based on their
// BufOffset. active.Valid=false means no match is currently active — every
// matching cell is then plain Match (§1.7: no -1 sentinel comparison).
// D5: unexported — no caller outside this package.
func applyMatchOverlay(cells []Cell, matches []SelInterval, active ActiveMatch) {
	for i := range cells {
		if cells[i].BufOffset < 0 {
			continue
		}
		for mi, m := range matches {
			if cells[i].BufOffset >= m.Start && cells[i].BufOffset < m.End {
				if active.Valid && mi == active.Index {
					cells[i].ActiveMatch = true
				} else {
					cells[i].Match = true
				}
				break
			}
		}
	}
}

// cellEffectiveStyle computes the final style for a cell considering overlays.
// Precedence: Cursor > ActiveMatch > Selected > Match > base style.
// When dim is set, Faint(true) is folded into the result so each style run emits
// the faint attribute as part of its own SGR sequence.
func cellEffectiveStyle(c Cell, selStyle, cursorStyle, matchStyle, activeMatchStyle lipgloss.Style, dim bool) lipgloss.Style {
	s := c.Style
	switch {
	case c.Cursor:
		s = cursorStyle
	case c.ActiveMatch:
		s = c.Style.Background(activeMatchStyle.GetBackground())
	case c.Selected:
		s = c.Style.Background(selStyle.GetBackground())
	case c.Match:
		s = c.Style.Background(matchStyle.GetBackground())
	}
	if dim {
		s = s.Faint(true)
	}
	return s
}

// stylesEqual compares two lipgloss styles for equality.
func stylesEqual(a, b lipgloss.Style) bool {
	return a.Render(" ") == b.Render(" ")
}
