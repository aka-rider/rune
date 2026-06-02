package editor

import (
	"strings"
	"unicode/utf8"

	"charm.land/lipgloss/v2"
	"github.com/mattn/go-runewidth"

	"rune/pkg/editor/display"
)

// Cell represents a single visual character in the cell grid.
// The cell grid is an intermediate representation between span rendering and
// final ANSI string output, enabling unified cursor/selection overlay on all spans.
type Cell struct {
	Rune      rune
	Grapheme  string         // multi-codepoint cluster (e.g. Kitty placeholder + diacritics); when non-empty, emitted verbatim instead of Rune
	Width     int            // visual width: 1 for normal, 2 for CJK/fullwidth
	Style     lipgloss.Style // base style (syntax color, bold, etc.)
	BufOffset int            // buffer byte offset (-1 for decorative/padding)
	Selected  bool
	Cursor    bool
}

// spanToCells converts a DisplaySpan into a slice of Cells with correct BufOffset
// per cell. For Revealed spans (CellMap nil), offset is trivially BufferStart + bytePos.
// For Rendered spans with a CellMap, offsets come from the map.
func spanToCells(sp display.DisplaySpan, baseStyle lipgloss.Style) []Cell {
	if sp.State == display.Revealed || sp.CellMap == nil {
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
	cells := make([]Cell, 0, len(text))
	pos := 0
	mapIdx := 0
	for pos < len(text) {
		r, size := utf8.DecodeRuneInString(text[pos:])
		if size == 0 {
			size = 1
			r = utf8.RuneError
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
		// Advance mapIdx by byte count (CellMap is per-byte for the rendered text)
		mapIdx += size
		pos += size
	}
	return cells
}

// spanToCellsStyled converts a span to styled cells, applying kind-specific
// styling (bold, inline code, headings, etc.) and syntax highlighting for code fences.
// For table spans with inline formatting (bold, link, etc.), the table role style
// is merged with the kind-specific style.
func (m Model) spanToCellsStyled(sp display.DisplaySpan) []Cell {
	if sp.State == display.Revealed {
		// Revealed spans have no special styling from the renderer
		return spanToCells(sp, lipgloss.NewStyle())
	}

	// Link kinds in table context: merge table role style with link style.
	if sp.TableRole != 0 {
		switch sp.LinkRole() {
		case display.LinkRoleImage:
			// Alt-text fallback; the image paints separately via inline placement.
			return spanToCells(sp, lipgloss.NewStyle())
		case display.LinkRoleNavigable:
			style := m.tableRoleStyle(sp.TableRole)
			return spanToCells(sp, m.styles.Link.Background(style.GetBackground()))
		}
	}

	// Non-table link kinds dispatch by unified role before the kind switch.
	switch sp.LinkRole() {
	case display.LinkRoleImage:
		// Alt-text fallback; the image paints separately via inline placement.
		return spanToCells(sp, lipgloss.NewStyle())
	case display.LinkRoleNavigable:
		return spanToCells(sp, m.styles.Link)
	}

	// Table spans: merge table role style with kind-specific inline formatting.
	if sp.TableRole != 0 {
		return spanToCells(sp, m.mergeTableStyle(sp.TableRole, sp.Kind))
	}

	switch sp.Kind {
	case display.TokenCodeFence:
		return m.codeFenceSpanToCells(sp)
	case display.TokenHeading:
		return spanToCells(sp, m.headingStyle(sp.HeadingLevel))
	case display.TokenInlineCode:
		return spanToCells(sp, m.styles.InlineCode)
	case display.TokenBold:
		return spanToCells(sp, m.styles.MdBold)
	case display.TokenItalic:
		return spanToCells(sp, m.styles.MdItalic)
	case display.TokenStrikethrough:
		return spanToCells(sp, m.styles.MdStrikethrough)
	case display.TokenBlockquote:
		return spanToCells(sp, m.styles.MdBlockquote)
	case display.TokenHorizontalRule:
		return spanToCells(sp, m.styles.HorizontalRule)
	case display.TokenTag:
		return spanToCells(sp, m.styles.Tag)
	case display.TokenListMarker:
		return spanToCells(sp, m.styles.ListMarker)
	case display.TokenTaskList:
		return m.taskListSpanToCells(sp)
	case display.TokenTable:
		return m.tableSpanToCells(sp)
	default:
		return spanToCells(sp, lipgloss.NewStyle())
	}
}

// tableRoleStyle returns the base style for a table role (header, separator, body).
func (m Model) tableRoleStyle(role display.TableRoleKind) lipgloss.Style {
	switch role {
	case display.TableRoleHeader:
		return m.styles.TableHeader
	case display.TableRoleSeparator:
		return m.styles.TableSeparator
	default:
		return m.styles.TableBody
	}
}

// mergeTableStyle merges a table role style with kind-specific inline formatting.
// This enables bold, italic, inline code, etc. inside table cells while preserving
// the table's background color and other role-specific styling.
func (m Model) mergeTableStyle(role display.TableRoleKind, kind display.TokenKind) lipgloss.Style {
	base := m.tableRoleStyle(role)

	switch kind {
	case display.TokenBold:
		return base.Bold(true)
	case display.TokenItalic:
		return base.Italic(true)
	case display.TokenStrikethrough:
		return base.Strikethrough(true)
	case display.TokenInlineCode:
		// Inline code in tables: use inline code foreground with table background
		return m.styles.InlineCode.Background(base.GetBackground())
	case display.TokenLink:
		// Link in tables: use link style with table background
		return m.styles.Link.Background(base.GetBackground())
	case display.TokenWikiLink:
		return m.styles.Link.Background(base.GetBackground())
	default:
		return base
	}
}

// headingStyle returns the appropriate style for a heading level.
func (m Model) headingStyle(level int) lipgloss.Style {
	switch level {
	case 1:
		return m.styles.HeadingH1
	case 2:
		return m.styles.HeadingH2
	case 3:
		return m.styles.HeadingH3
	case 4:
		return m.styles.HeadingH4
	case 5:
		return m.styles.HeadingH5
	case 6:
		return m.styles.HeadingH6
	default:
		return m.styles.HeadingH6
	}
}

// taskListSpanToCells creates styled cells for a task list checkbox span.
func (m Model) taskListSpanToCells(sp display.DisplaySpan) []Cell {
	style := m.styles.TaskUnchecked
	if strings.Contains(sp.Text, "☑") {
		style = m.styles.TaskChecked
	}
	return spanToCells(sp, style)
}

// tableSpanToCells creates styled cells for a table span based on its role.
func (m Model) tableSpanToCells(sp display.DisplaySpan) []Cell {
	switch sp.TableRole {
	case display.TableRoleHeader:
		return spanToCells(sp, m.styles.TableHeader)
	case display.TableRoleSeparator:
		return spanToCells(sp, m.styles.TableSeparator)
	default:
		return spanToCells(sp, m.styles.TableBody)
	}
}

// codeFenceSpanToCells applies Chroma syntax highlighting to code fence content.
func (m Model) codeFenceSpanToCells(sp display.DisplaySpan) []Cell {
	// Fence marker lines (opening/closing) have empty text in rendered mode.
	if sp.Text == "" {
		if sp.Language != "" {
			label := sp.Language
			cells := make([]Cell, 0, len(label))
			pos := 0
			for pos < len(label) {
				r, size := utf8.DecodeRuneInString(label[pos:])
				if size == 0 {
					size = 1
				}
				cells = append(cells, Cell{
					Rune:      r,
					Width:     runewidth.RuneWidth(r),
					Style:     m.styles.CodeBlockLabel,
					BufOffset: -1,
				})
				pos += size
			}
			return cells
		}
		return nil
	}

	// No highlighter or no language — use plain code style
	if m.highlighter == nil || sp.Language == "" {
		return spanToCells(sp, m.styles.CodePlain)
	}

	spans, err := m.highlighter(sp.Language, sp.Text)
	if err != nil || spans == nil {
		return spanToCells(sp, m.styles.CodePlain)
	}

	// Apply per-token styles from Chroma while preserving buffer offsets.
	// Code fence content is 1:1 with buffer bytes (CellMap nil), so use BufferStart + pos.
	cells := make([]Cell, 0, len(sp.Text))
	bufPos := sp.BufferStart
	for _, hs := range spans {
		style := classToStyle(hs.Class, m.styles)
		pos := 0
		for pos < len(hs.Text) {
			r, size := utf8.DecodeRuneInString(hs.Text[pos:])
			if size == 0 {
				size = 1
				r = utf8.RuneError
			}
			w := runewidth.RuneWidth(r)
			if w == 0 {
				w = 1
			}
			cells = append(cells, Cell{
				Rune:      r,
				Width:     w,
				Style:     style,
				BufOffset: bufPos,
			})
			pos += size
			bufPos += size
		}
	}
	return cells
}

// applyOverlays marks cells as Selected or Cursor based on their BufOffset.
func applyOverlays(cells []Cell, cursorOffsets map[int]bool, selections []selInterval) {
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

// sliceCells performs horizontal scrolling at the cell level.
// It skips cells until scrollCol visual columns are consumed, then takes cells
// until viewWidth visual columns are filled. Wide chars at the left edge that
// would be partially visible are replaced with a space padding cell.
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
	// Skip cells before scrollCol
	for i < len(cells) {
		if col+cells[i].Width > scrollCol {
			// This cell crosses the left boundary
			if col < scrollCol {
				// Partially visible wide char — emit padding
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

	// Collect cells within the viewport
	usedWidth := col - scrollCol
	for i < len(cells) && usedWidth < viewWidth {
		c := cells[i]
		if usedWidth+c.Width > viewWidth {
			// Wide char at right edge would overflow — skip or pad
			break
		}
		result = append(result, c)
		usedWidth += c.Width
		i++
	}

	return result
}

// cellsToString converts a slice of cells to a final ANSI-styled string.
// It groups consecutive cells with the same effective style for efficiency.
func cellsToString(cells []Cell, selStyle, cursorStyle lipgloss.Style) string {
	if len(cells) == 0 {
		return ""
	}

	var b strings.Builder
	b.Grow(len(cells) * 2) // rough estimate

	// Process cells, grouping by effective style
	i := 0
	for i < len(cells) {
		effectiveStyle := cellEffectiveStyle(cells[i], selStyle, cursorStyle)
		// Find run of cells with same effective style
		j := i + 1
		for j < len(cells) {
			nextStyle := cellEffectiveStyle(cells[j], selStyle, cursorStyle)
			if !stylesEqual(effectiveStyle, nextStyle) {
				break
			}
			j++
		}

		// Build the text for this run
		var run strings.Builder
		for k := i; k < j; k++ {
			if cells[k].Grapheme != "" {
				// Multi-codepoint cluster (e.g. Kitty image placeholder with
				// row/column diacritics) — emit verbatim so it is not split.
				run.WriteString(cells[k].Grapheme)
				continue
			}
			run.WriteRune(cells[k].Rune)
		}

		// Render with style
		styled := effectiveStyle.Render(run.String())
		b.WriteString(styled)
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
		// Merge selection background onto the cell's base style
		return c.Style.Background(selStyle.GetBackground())
	}
	return c.Style
}

// stylesEqual compares two lipgloss styles for equality by their rendered output
// on an empty string. This is a pragmatic approach since lipgloss doesn't expose
// a direct equality check.
func stylesEqual(a, b lipgloss.Style) bool {
	return a.Render("") == b.Render("")
}
