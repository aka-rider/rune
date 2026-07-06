package textedit

import (
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/display"
	"rune/pkg/ui/scroll"
)

// ViewportState is the scroll position of the visible content window.
type ViewportState struct {
	TopRow    int
	ScrollCol int
}

func (m Model) contentHeight() int {
	h := m.height - m.headerHeight
	if h < 1 {
		return 1
	}
	return h
}

// ContentHeight returns the allocated content height.
func (m Model) ContentHeight() int { return m.contentHeight() }

// Viewport returns the viewport state.
func (m Model) Viewport() ViewportState { return m.viewport }

// clampScroll clamps viewport.TopRow to [0, maxTop].
func (m Model) clampScroll() Model {
	maxTop := m.snapshot.TotalRows - m.contentHeight()
	if maxTop < 0 {
		maxTop = 0
	}
	if m.viewport.TopRow < 0 {
		m.viewport.TopRow = 0
	}
	if m.viewport.TopRow > maxTop {
		m.viewport.TopRow = maxTop
	}
	return m
}

// AtBottom reports whether the viewport is at the bottom of the content.
func (m Model) AtBottom() bool {
	return m.viewport.TopRow >= m.snapshot.TotalRows-m.contentHeight()
}

// GotoBottom scrolls to the bottom of the content.
func (m Model) GotoBottom() Model {
	m.viewport.TopRow = max(0, m.snapshot.TotalRows-m.contentHeight())
	return m
}

// ScrollOffset returns the current TopRow.
func (m Model) ScrollOffset() int { return m.viewport.TopRow }

// SetScrollOffset sets TopRow, clamped to [0, maxTop].
func (m Model) SetScrollOffset(offset int) Model {
	maxTop := m.snapshot.TotalRows - m.contentHeight()
	if maxTop < 0 {
		maxTop = 0
	}
	if offset < 0 {
		offset = 0
	}
	if offset > maxTop {
		offset = maxTop
	}
	m.viewport.TopRow = offset
	return m
}

// NaturalContentHeight returns the visual height of content at the given width.
func (m Model) NaturalContentHeight(width int) int {
	sm := display.NewSyntaxMap()
	wm := display.NewWrapMap(0)
	sm = sm.SetWidth(width)
	sm, snapSnap := sm.Sync(m.buf, cursor.NewCursorSet(0))
	wm = wm.SetWidth(width)
	snap := display.BuildSnapshot(wm.Sync(snapSnap))
	snap = display.ExpandTableRows(snap)
	return snap.TotalRows
}

// ScrollToCursor scrolls the viewport to make the primary cursor visible.
func (m Model) ScrollToCursor() Model {
	if len(m.cursors.All()) == 0 {
		return m
	}
	primary := m.cursors.Primary()
	bp := m.buf.OffsetToLineCol(primary.Position)
	sp := m.syntaxSnap.BufferToSyntax(bp)
	wp := m.wrapSnap.SyntaxToWrap(sp)

	contentH := m.contentHeight()

	// Map wrap-row to display-row (accounting for image/table expansion)
	modelLine := sp.Line
	wrapOffsetWithinLine := wp.Row - m.wrapSnap.ModelLineToFirstRow(modelLine)
	if wrapOffsetWithinLine < 0 {
		wrapOffsetWithinLine = 0
	}
	cursorDisplayRow := m.snapshot.ModelLineToFirstRow(modelLine) + wrapOffsetWithinLine

	vMargin := min(4, contentH/4)
	m.viewport.TopRow = scroll.Follow(
		cursorDisplayRow, m.viewport.TopRow, contentH, m.snapshot.TotalRows, vMargin, 0)

	// Horizontal scroll
	if !m.softWrap {
		hMargin := min(4, m.width/4)
		hJump := m.width / 4
		// Use cursor col + viewport width as a sentinel total so the viewport
		// never scrolls past the cursor's column (true line width unknown without
		// iterating spans; this preserves existing end-of-line behavior).
		lineWidth := wp.Col + m.width
		m.viewport.ScrollCol = scroll.Follow(
			wp.Col, m.viewport.ScrollCol, m.width, lineWidth, hMargin, hJump)
	}

	return m
}
