package markdownedit

import (
	"time"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/editor/coords"
	"rune/pkg/editor/display"
)

const (
	mouseScrollLines    = 3
	multiClickThreshold = 500 * time.Millisecond
)

type mouseState struct {
	lastClickTime time.Time
	lastClickX    int
	lastClickY    int
	clickCount    int
	dragging      bool
	dragAnchor    int
}

func (m Model) handleMouseClick(msg tea.MouseClickMsg, now time.Time) (Model, tea.Cmd) {
	if !m.Model.Focused() {
		return m, nil
	}
	if msg.Button != tea.MouseLeft {
		return m, nil
	}

	dp := coords.DisplayPoint{
		Row: msg.Y - m.Model.OffsetY(),
		Col: msg.X - m.Model.OffsetX(),
	}
	if dp.Row < 0 {
		return m, nil
	}
	if dp.Col < 0 {
		dp.Col = 0
	}

	snap := m.Model.Snapshot()
	vp := m.Model.Viewport()

	// Skip clicks that land on image-reserved display rows.
	displayRow := dp.Row + vp.TopRow
	if displayRow >= 0 && displayRow < len(snap.Lines) {
		if snap.Lines[displayRow].ImagePath != "" {
			return m, nil
		}
	}

	bp, offset := m.Model.DisplayToBuffer(dp)

	// Check if click is on a link span — only when not read-only.
	if !m.Model.ReadOnly() {
		if linkPath := m.resolveLinkClick(bp, offset); linkPath != "" {
			m.Model = m.Model.MousePositionCursor(offset)
			m.mouse.dragAnchor = offset
			return m, func() tea.Msg { return LinkClickedMsg{Path: linkPath} }
		}
	}

	// Detect multi-click
	clickCount := 1
	if now.Sub(m.mouse.lastClickTime) < multiClickThreshold &&
		msg.X == m.mouse.lastClickX && msg.Y == m.mouse.lastClickY {
		clickCount = m.mouse.clickCount + 1
	}
	m.mouse.lastClickTime = now
	m.mouse.lastClickX = msg.X
	m.mouse.lastClickY = msg.Y
	m.mouse.clickCount = clickCount
	m.mouse.dragging = false

	switch {
	case msg.Mod&tea.ModAlt != 0:
		m.Model = m.Model.MouseAddCursor(offset)
	case msg.Mod&tea.ModShift != 0:
		m.Model = m.Model.MouseExtendSelection(offset)
	case clickCount >= 3:
		m.Model = m.Model.MouseSelectLine(bp.Line)
	case clickCount == 2:
		m.Model = m.Model.MouseSelectWord(offset)
	default:
		m.Model = m.Model.MousePositionCursor(offset)
		m.mouse.dragAnchor = offset
	}

	return m, nil
}

func (m Model) resolveLinkClick(bp coords.BufferPoint, offset int) string {
	ss := m.Model.SyntaxSnap()
	if bp.Line < 0 || bp.Line >= len(ss.Lines) {
		return ""
	}

	line := ss.Lines[bp.Line]
	col := offset

	for _, sp := range line.Spans {
		if sp.BufferStart > col {
			break
		}
		if col >= sp.BufferEnd {
			continue
		}

		switch sp.LinkRole() {
		case display.LinkRoleImage:
			return ""
		case display.LinkRoleNavigable:
			if sp.Kind == display.TokenWikiLink {
				if sp.WikiLinkTarget == "" {
					return ""
				}
				return m.resolveNavigation(sp.WikiLinkTarget, true)
			}
			if sp.LinkURL == "" {
				return ""
			}
			return m.resolveNavigation(sp.LinkURL, false)
		}
	}
	return ""
}

func (m Model) handleMouseMotion(msg tea.MouseMotionMsg) (Model, tea.Cmd) {
	if !m.Model.Focused() {
		return m, nil
	}
	if msg.Button != tea.MouseLeft {
		return m, nil
	}

	dp := coords.DisplayPoint{
		Row: msg.Y - m.Model.OffsetY(),
		Col: msg.X - m.Model.OffsetX(),
	}
	if dp.Row < 0 {
		dp.Row = 0
	}
	if dp.Col < 0 {
		dp.Col = 0
	}

	_, offset := m.Model.DisplayToBuffer(dp)

	if !m.mouse.dragging {
		m.mouse.dragging = true
	}

	// Extend selection from the click anchor to the current drag position.
	m.Model = m.Model.MouseExtendSelection(offset)
	return m, nil
}

func (m Model) handleMouseWheel(msg tea.MouseWheelMsg) (Model, tea.Cmd) {
	if !m.Model.Focused() {
		return m, nil
	}

	vp := m.Model.Viewport()
	snap := m.Model.Snapshot()

	switch msg.Button {
	case tea.MouseWheelUp:
		vp.TopRow -= mouseScrollLines
		if vp.TopRow < 0 {
			vp.TopRow = 0
		}
	case tea.MouseWheelDown:
		vp.TopRow += mouseScrollLines
		maxTop := snap.TotalRows - m.Model.ContentHeight()
		if maxTop < 0 {
			maxTop = 0
		}
		if vp.TopRow > maxTop {
			vp.TopRow = maxTop
		}
	}

	m.Model = m.Model.SetScrollOffset(vp.TopRow)

	// Re-arm animation ticks for images scrolled into view.
	var cmd tea.Cmd
	m, cmd = m.armImageTicks()
	return m, cmd
}
