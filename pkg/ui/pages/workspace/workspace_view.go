package workspace

import (
	"strings"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"rune/pkg/ui/components/textedit"
	"rune/pkg/ui/styles"
)

func (m Model) recalcLayout() Model {
	contentH := m.totalHeight - m.footer.Height()
	if contentH < 0 {
		contentH = 0
	}

	leftW := 0
	if m.leftVisible {
		leftW = m.leftPaneW
	}
	rightW := 0
	if m.rightVisible {
		rightW = m.rightPaneW
	}
	centerW := m.totalWidth - leftW - rightW
	if centerW < 0 {
		centerW = 0
	}

	innerH := contentH - 2
	if innerH < 0 {
		innerH = 0
	}
	innerLeftW := leftW - 2
	if innerLeftW < 0 {
		innerLeftW = 0
	}
	innerCenterW := centerW - 2
	if innerCenterW < 0 {
		innerCenterW = 0
	}
	innerRightW := rightW - 2
	if innerRightW < 0 {
		innerRightW = 0
	}

	// Title occupies the first row inside the center pane border (D6)
	titleH := m.title.Height()
	searchH := m.search.Height() // 0 when not visible; 1 when visible
	editorH := innerH - titleH - searchH
	if editorH < 0 {
		editorH = 0
	}

	otH := m.opentabs.Height()
	available := innerH - otH
	if available < 0 {
		available = 0
	}
	ftH := available
	if ftH < 4 {
		ftH = 4
	}
	if ftH > available {
		ftH = available
	}
	m.filetree = m.filetree.SetSize(innerLeftW, ftH)
	m.opentabs = m.opentabs.SetSize(innerLeftW, otH)
	m.opentabs = m.opentabs.SetOffset(1, ftH+1)

	m.title = m.title.SetSize(innerCenterW, titleH)
	m.search = m.search.SetSize(innerCenterW, searchH)
	m.breadcrumb = m.breadcrumb.SetSize(centerW, 1)

	topOffset := 1
	if m.err != nil {
		topOffset = 2
	}

	m.editor = m.editor.SetRect(textedit.Rect{
		X: leftW + 1,
		Y: topOffset + titleH + searchH,
		W: innerCenterW,
		H: editorH,
	})

	m.chat = m.chat.SetSize(innerRightW, innerH)
	m.footer = m.footer.SetSize(m.totalWidth, m.footer.Height())

	m.filetree = m.filetree.SetOffset(1, 1)

	return m
}

func (m Model) paneAtPoint(x, y int) (pane, bool) {
	contentH := m.totalHeight - m.footer.Height()
	if y >= contentH {
		return 0, false
	}

	if m.leftVisible && x < m.leftPaneW {
		innerH := contentH - 2
		otH := m.opentabs.Height()
		avail := innerH - otH
		if avail < 0 {
			avail = 0
		}
		ftH := avail
		if ftH < 4 {
			ftH = 4
		}
		if ftH > avail {
			ftH = avail
		}
		if y > ftH {
			return paneTabs, true
		}
		return paneTree, true
	}
	rightStart := m.totalWidth - m.rightPaneW
	if m.rightVisible && x >= rightStart {
		return paneChat, true
	}

	topOffset := 1
	if m.err != nil {
		topOffset = 2
	}
	if y == topOffset {
		return paneTitle, true
	}
	if m.search.Visible() && y == topOffset+m.title.Height() {
		return paneSearch, true
	}
	return paneCenter, true
}

func (m Model) dividerAtPoint(x, y int) (dragState, bool) {
	contentH := m.totalHeight - m.footer.Height()
	if y < 0 || y >= contentH {
		return dragNone, false
	}

	if m.leftVisible {
		if x == m.leftPaneW-1 || x == m.leftPaneW {
			return dragLeft, true
		}
	} else {
		if x == 0 {
			return dragLeft, true
		}
	}

	if m.rightVisible {
		rightStart := m.totalWidth - m.rightPaneW
		if x == rightStart-1 || x == rightStart {
			return dragRight, true
		}
	} else {
		if x == m.totalWidth-1 {
			return dragRight, true
		}
	}

	return dragNone, false
}

func (m Model) View() tea.View {
	if m.totalWidth == 0 {
		return tea.NewView("")
	}

	contentH := m.totalHeight - m.footer.Height()
	if contentH < 0 {
		contentH = 0
	}
	leftW := 0
	if m.leftVisible {
		leftW = m.leftPaneW
	}
	rightW := 0
	if m.rightVisible {
		rightW = m.rightPaneW
	}
	centerW := m.totalWidth - leftW - rightW
	if centerW < 0 {
		centerW = 0
	}

	var centerParts []string
	centerParts = append(centerParts, m.title.View())
	if m.search.Visible() {
		centerParts = append(centerParts, m.search.View())
	}
	centerParts = append(centerParts, m.editor.View())
	centerContent := lipgloss.JoinVertical(lipgloss.Left, centerParts...)
	centerBlock := borderStyle(m.focus.isCenter(), m.styles).
		Width(centerW).Height(contentH).
		Render(centerContent)
	centerBlock = overlayBreadcrumb(centerBlock, m.breadcrumb.View(), m.focus.isCenter(), m.styles)

	var chatBlock string
	if m.rightVisible {
		chatBlock = borderStyle(m.focus == paneChat, m.styles).
			Width(rightW).Height(contentH).
			Render(m.chat.View())
	}

	var body string
	switch {
	case m.leftVisible && m.rightVisible:
		leftContent := lipgloss.JoinVertical(lipgloss.Left,
			m.filetree.View(),
			m.opentabs.View(),
		)
		leftBlock := borderStyle(m.focus.isLeft(), m.styles).
			Width(leftW).Height(contentH).
			Render(leftContent)
		body = lipgloss.JoinHorizontal(lipgloss.Top, leftBlock, centerBlock, chatBlock)

	case m.leftVisible && !m.rightVisible:
		leftContent := lipgloss.JoinVertical(lipgloss.Left,
			m.filetree.View(),
			m.opentabs.View(),
		)
		leftBlock := borderStyle(m.focus.isLeft(), m.styles).
			Width(leftW).Height(contentH).
			Render(leftContent)
		body = lipgloss.JoinHorizontal(lipgloss.Top, leftBlock, centerBlock)

	case !m.leftVisible && m.rightVisible:
		body = lipgloss.JoinHorizontal(lipgloss.Top, centerBlock, chatBlock)

	default: // zen mode
		body = centerBlock
	}

	clamp := lipgloss.NewStyle().MaxWidth(m.totalWidth).MaxHeight(m.totalHeight)
	if m.err != nil {
		errLine := m.styles.Error.Render("error: " + m.err.Error())
		frame := lipgloss.JoinVertical(lipgloss.Left, errLine, body, m.footer.View())
		return tea.NewView(clamp.Render(frame))
	}
	frame := lipgloss.JoinVertical(lipgloss.Left, body, m.footer.View())
	return tea.NewView(clamp.Render(frame))
}

func overlayBreadcrumb(block, crumb string, active bool, st styles.Styles) string {
	if crumb == "" {
		return block
	}

	lines := strings.Split(block, "\n")
	if len(lines) < 2 {
		return block
	}

	lastIdx := len(lines) - 1
	bottomLine := lines[lastIdx]
	borderW := lipgloss.Width(bottomLine)

	bcWidth := lipgloss.Width(crumb)
	minOverhead := 7
	if bcWidth+minOverhead > borderW {
		return block
	}

	borderColor := st.InactiveBorder.GetBorderTopForeground()
	if active {
		borderColor = st.ActiveBorder.GetBorderTopForeground()
	}
	bStyle := lipgloss.NewStyle().Foreground(borderColor)

	rightPad := bStyle.Render("──╯")
	leftCorner := bStyle.Render("╰")
	content := " " + crumb + " "
	contentWidth := lipgloss.Width(content) + lipgloss.Width(rightPad) + lipgloss.Width(leftCorner)
	dashCount := borderW - contentWidth
	if dashCount < 0 {
		dashCount = 0
	}
	dashFill := bStyle.Render(strings.Repeat("─", dashCount))

	lines[lastIdx] = leftCorner + dashFill + content + rightPad
	return strings.Join(lines, "\n")
}

func borderStyle(active bool, st styles.Styles) lipgloss.Style {
	if active {
		return st.ActiveBorder
	}
	return st.InactiveBorder
}
