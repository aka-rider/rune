package textedit

import (
	"strings"

	"charm.land/lipgloss/v2"

	"rune/pkg/editor/display"
)

func (m Model) syncDisplay() Model {
	if m.syncFunc == nil {
		return m
	}
	width := m.width
	if width < 0 {
		width = 0
	}
	if m.wrapMap == (display.WrapMap{}) {
		m.wrapMap = display.NewWrapMap(0)
	}
	m.syntaxMap, m.syntaxSnap = m.syncFunc(m.buf, m.syntaxMap, m.cursors, m.focused, width)
	m.wrapMap = m.wrapMap.SetWidth(width)
	m.wrapSnap = m.wrapMap.Sync(m.syntaxSnap)
	m.snapshot = display.BuildSnapshot(m.wrapSnap)
	m.snapshot = display.ExpandTableRows(m.snapshot)
	if len(m.imageDims) > 0 {
		m.snapshot = display.ExpandImageRows(m.snapshot, m.imageDimsForPath)
	}
	return m
}

// imageDimsForPath returns the reserved cell footprint for a standalone image
// line. Unknown paths collapse to a single row, which ExpandImageRows treats as
// a no-op — so images that are not yet decoded/transmitted occupy one row.
func (m Model) imageDimsForPath(path string) display.ImageDims {
	if d, ok := m.imageDims[path]; ok {
		return d
	}
	return display.ImageDims{Cols: 0, Rows: 1}
}

// ---- View ----
//
// renderCells/RenderView are the shared rendering pipeline behind BOTH
// textedit's own View() and markdownedit's (markdownedit/view.go) — before
// this chokepoint the two carried near-verbatim duplicate copies of this
// entire pipeline. Two seams thread markdownedit's extra concerns through
// without textedit knowing anything about markdown or images:
//
//   - CellBuilderFunc: converts one span into styled cells. textedit's own
//     View() uses the plain default (SpanToCells with a zero-value base
//     style); markdownedit passes its spanToCellsStyled (markdown syntax
//     highlighting).
//   - ImageRowFunc: renders one image-embed line (DisplayLine.ImagePath !=
//     "") as a complete cell row, bypassing span conversion, the
//     background-interval overlay, the EOL cursor, and cursor/selection/
//     match overlays entirely — an image row carries no buffer content of
//     its own to overlay onto. textedit's own View() passes nil (plain
//     textedit has no images, so no line's ImagePath is ever non-empty
//     anyway); markdownedit passes a closure over its own image/terminal-
//     capability state.
//
// Preserved exactly across the unification: image-row emission modes
// (Kitty live/pending vs iTerm2 reserved-space), match overlay applied
// unconditionally (never gated on focus — every textedit instance without
// markdownedit's search wiring has an always-empty m.searchMatches, so this
// is a no-op for them), dim-when-unfocused-except-while-search-matches-are-
// shown, image rows NEVER dimmed regardless of focus (Faint must never touch
// the Kitty unicode-placeholder's precise 24-bit foreground color, which
// encodes the image ID), tilde fill, and the outer
// MaxWidth/MaxHeight/Width/Height wrap.

// CellBuilderFunc converts one syntax span into styled cells.
type CellBuilderFunc func(sp display.DisplaySpan) []Cell

// defaultCellBuilder is textedit's own plain (unstyled) cell builder.
func defaultCellBuilder(sp display.DisplaySpan) []Cell {
	return SpanToCells(sp, lipgloss.NewStyle())
}

// ImageRowFunc renders one image-embed DisplayLine into a complete,
// UNSLICED cell row. ok=false means "not applicable here", so renderCells
// falls through to ordinary span rendering for that line.
type ImageRowFunc func(l display.DisplayLine) ([]Cell, bool)

// renderCells builds the 2D cell grid for the given content height.
// It is called by both View()/RenderView() and the gated FuzzCells()
// accessor so the fuzzer checks exactly what the terminal renders — no
// drift. cellBuilder nil defaults to defaultCellBuilder; imageRow nil means
// no line is ever image-handled (matches plain textedit, which has none).
func (m Model) renderCells(contentHeight int, cellBuilder CellBuilderFunc, imageRow ImageRowFunc) [][]Cell {
	lines := m.snapshot.Slice(m.viewport.TopRow, contentHeight)
	cells, _ := m.renderCellsForLines(lines, cellBuilder, imageRow)
	return cells
}

// renderCellsForLines is renderCells' body, split out so RenderView can
// reuse the SAME already-sliced lines both to build cells and to decide,
// per rendered row, whether it was an image row (never dimmed). The second
// return value is imageHandled: imageHandled[i] is true iff row i was
// ACTUALLY rendered by imageRow (ok=true), never merely "this DisplayLine
// carries an ImagePath" — B4: keying dim-exemption on
// lines[i].ImagePath == "" instead let a hook that declines a row
// (ok=false, falling through to ordinary span rendering just below) wrongly
// escape dimming, since the line's ImagePath stays non-empty even though
// this function rendered it as plain text.
func (m Model) renderCellsForLines(lines []display.DisplayLine, cellBuilder CellBuilderFunc, imageRow ImageRowFunc) ([][]Cell, []bool) {
	if cellBuilder == nil {
		cellBuilder = defaultCellBuilder
	}

	cursorOffsets := make(map[int]bool)
	if m.focused && !m.readOnly {
		for _, c := range m.cursors.All() {
			cursorOffsets[c.Position] = true
		}
	}
	var selections []SelInterval
	if m.focused || m.readOnly {
		selections = m.Selections()
	}

	result := make([][]Cell, len(lines))
	imageHandled := make([]bool, len(lines))
	for i, l := range lines {
		if imageRow != nil && l.ImagePath != "" {
			if cells, ok := imageRow(l); ok {
				result[i] = sliceCells(cells, m.viewport.ScrollCol, m.width)
				imageHandled[i] = true
				continue
			}
		}

		// Convert all spans to cells
		var lineCells []Cell
		for _, sp := range l.Spans {
			lineCells = append(lineCells, cellBuilder(sp)...)
		}

		// Background-interval overlay (merge-diff coloring, etc.) — applied BEFORE
		// sliceCells so the tint is part of the base cell style; cursor/selection
		// overlays below still take precedence (cellEffectiveStyle).
		if len(m.bgIntervals) > 0 {
			ApplyBackgroundIntervals(lineCells, m.bgIntervals)
		}

		// Horizontal scrolling at cell level
		lineCells = sliceCells(lineCells, m.viewport.ScrollCol, m.width)

		// EOL cursor: append a synthetic cell when the cursor sits at the end-of-line
		// position (lineEnd = last span's BufferEnd). Added AFTER sliceCells so it
		// survives even when the preceding content fills the viewport exactly (e.g. a
		// double-width CJK character at the last column pushes usedWidth to m.width and
		// causes the loop in sliceCells to exit before processing this cell).
		if m.focused && !m.readOnly {
			lineEnd := 0
			if len(l.Spans) > 0 {
				last := l.Spans[len(l.Spans)-1]
				lineEnd = last.BufferEnd
			}
			for off := range cursorOffsets {
				if off == lineEnd {
					isLastVisible := i+1 >= len(lines) || lines[i+1].ModelLine != l.ModelLine
					if isLastVisible {
						lineCells = append(lineCells, Cell{
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

		// Apply cursor and selection overlays
		if m.focused && (len(cursorOffsets) > 0 || len(selections) > 0) {
			applyOverlays(lineCells, cursorOffsets, selections)
		}

		// Apply match overlay unconditionally (not gated on focused).
		if len(m.searchMatches) > 0 {
			applyMatchOverlay(lineCells, m.searchMatches, m.searchActive)
		}

		result[i] = lineCells
	}
	return result, imageHandled
}

func (m Model) View() string {
	return m.RenderView(nil, nil)
}

// RenderView is the shared render pipeline described in the doc comment
// above renderCells: cell-grid build + per-line string conversion + tilde
// fill + outer wrap. See that comment for the exact invariants preserved.
func (m Model) RenderView(cellBuilder CellBuilderFunc, imageRow ImageRowFunc) string {
	if m.width == 0 || m.height == 0 {
		return ""
	}

	contentHeight := m.contentHeight()
	lines := m.snapshot.Slice(m.viewport.TopRow, contentHeight)
	cells, imageHandled := m.renderCellsForLines(lines, cellBuilder, imageRow)

	cursorStyle := lipgloss.NewStyle().Reverse(true)
	selStyle := m.styles.Selection
	matchStyle := m.styles.SearchMatch
	activeMatchStyle := m.styles.SearchActiveMatch

	// Dim unfocused content, except while search matches are shown (keep
	// highlights legible). Applied per style run inside cellsToString.
	dim := !m.focused && len(m.searchMatches) == 0

	var renderedLines []string
	for i, lineCells := range cells {
		// Image rows are never dimmed, regardless of focus: Faint must never
		// touch the Kitty unicode-placeholder's precise 24-bit foreground
		// color, which encodes the image ID. B4: keyed on imageHandled[i] —
		// whether THIS row was actually rendered by imageRow (ok=true) —
		// never lines[i].ImagePath == "", which stays non-empty even when the
		// hook declines the row (ok=false) and it falls through to ordinary
		// (dimmable) span rendering above.
		lineDim := dim && !imageHandled[i]
		renderedLines = append(renderedLines, cellsToString(lineCells, selStyle, cursorStyle, matchStyle, activeMatchStyle, lineDim))
	}

	// Filler tildes carry no embedded ANSI, so a single faint render is correct.
	tilde := "~"
	if dim {
		tilde = lipgloss.NewStyle().Faint(true).Render("~")
	}
	for len(renderedLines) < contentHeight {
		renderedLines = append(renderedLines, tilde)
	}

	// Dimming is folded per style run inside cellsToString — no post-hoc Faint
	// wrap, which would be cleared by inner styled runs' embedded resets.
	content := strings.Join(renderedLines, "\n")

	return lipgloss.NewStyle().
		MaxWidth(m.width).
		MaxHeight(m.height).
		Width(m.width).
		Height(m.height).
		Render(content)
}
