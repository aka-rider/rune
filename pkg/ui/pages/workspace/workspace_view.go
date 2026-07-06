package workspace

import (
	"strings"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"rune/pkg/ui/components/textedit"
	"rune/pkg/ui/pages/workspace/mergemode"
	"rune/pkg/ui/styles"
)

// paneGeometry is the shared top-level layout — the SAME numbers recalcLayout
// uses to size every child AND paneAtPoint/dividerAtPoint use to hit-test the
// mouse. Before this chokepoint, recalcLayout/paneAtPoint/dividerAtPoint/View
// each independently recomputed contentH/leftW/rightW/centerW, and
// recalcLayout/paneAtPoint each independently recomputed the filetree/
// opentabs height split — four (resp. two) copies that could silently drift
// apart, misrouting a mouse click to the wrong pane/row (critic: "layout vs
// mouse hit-testing must agree").
type paneGeometry struct {
	ContentH   int // total height available above the footer
	InnerH     int // ContentH - 2 (border top+bottom), clamped at 0 — the bordered interior height shared by every pane (D8: folds recalcLayout's own duplicate "contentH-2" clamp into this one computation)
	LeftW      int // 0 when the left pane is hidden
	CenterW    int
	RightW     int // 0 when the right pane is hidden
	RightStart int // m.totalWidth - m.rightPaneW — the right pane's X REGARDLESS of visibility (dividerAtPoint's hidden-pane restore zone needs this even when RightW is 0)
	FiletreeH  int // filetree height within the left column's bordered interior
	OpentabsH  int // opentabs height within the left column's bordered interior
}

// paneGeometry computes the shared geometry described by the paneGeometry
// type above — the one place recalcLayout/paneAtPoint/dividerAtPoint/View
// all read it from.
func (m Model) paneGeometry() paneGeometry {
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

	return paneGeometry{
		ContentH:   contentH,
		InnerH:     innerH,
		LeftW:      leftW,
		CenterW:    centerW,
		RightW:     rightW,
		RightStart: m.totalWidth - m.rightPaneW,
		FiletreeH:  ftH,
		OpentabsH:  otH,
	}
}

func (m Model) recalcLayout() Model {
	g := m.paneGeometry()
	leftW := g.LeftW
	rightW := g.RightW
	centerW := g.CenterW
	innerH := g.InnerH

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

	otH := g.OpentabsH
	ftH := g.FiletreeH
	m.filetree = m.filetree.SetSize(innerLeftW, ftH)
	m.opentabs = m.opentabs.SetSize(innerLeftW, otH)
	m.opentabs = m.opentabs.SetOffset(1, ftH+1)

	m.title = m.title.SetSize(innerCenterW, titleH)
	m.search = m.search.SetSize(innerCenterW, searchH)
	m.breadcrumb = m.breadcrumb.SetSize(centerW, 1)

	// The error banner (§4) is composed INTO body's existing top row in
	// View() (overlayErrorLine) rather than adding a row above it, so no
	// geometry here shifts when m.err is set — topOffset is always 1.
	topOffset := 1

	m.editor = m.editor.SetRect(textedit.Rect{
		X: leftW + 1,
		Y: topOffset + titleH + searchH,
		W: innerCenterW,
		H: editorH,
	})
	// The merge view substitutes for the main editor in the center pane while
	// active (§4) — same box, so it never jumps size when merge toggles.
	m.merge = mergemode.SetSize(m.merge, innerCenterW, editorH)

	m.chat = m.chat.SetSize(innerRightW, innerH)
	m.footer = m.footer.SetSize(m.totalWidth, m.footer.Height())

	m.filetree = m.filetree.SetOffset(1, 1)

	return m
}

func (m Model) paneAtPoint(x, y int) (pane, bool) {
	g := m.paneGeometry()
	if y >= g.ContentH {
		return 0, false
	}

	if m.leftVisible && x < m.leftPaneW {
		if y > g.FiletreeH {
			return paneTabs, true
		}
		return paneTree, true
	}
	if m.rightVisible && x >= g.RightStart {
		return paneChat, true
	}

	// Mirrors recalcLayout's topOffset (§4): the error banner overlays body's
	// existing top row (overlayErrorLine) rather than shifting it, so hit
	// testing never depends on m.err.
	topOffset := 1
	if y == topOffset {
		return paneTitle, true
	}
	if m.search.Visible() && y == topOffset+m.title.Height() {
		return paneSearch, true
	}
	return paneCenter, true
}

func (m Model) dividerAtPoint(x, y int) (dragState, bool) {
	g := m.paneGeometry()
	if y < 0 || y >= g.ContentH {
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
		if x == g.RightStart-1 || x == g.RightStart {
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

	g := m.paneGeometry()
	contentH := g.ContentH
	leftW := g.LeftW
	rightW := g.RightW
	centerW := g.CenterW

	var centerParts []string
	centerParts = append(centerParts, m.title.View())
	if m.search.Visible() {
		centerParts = append(centerParts, m.search.View())
	}
	editorView := m.editor.View()
	switch {
	case mergemode.IsActive(m.merge):
		// The merge resolver is active: substitute the read-only diff view for
		// the main editor (which is hidden — it holds the marker working
		// buffer, §3/§4). Title/breadcrumb stay so the doc identity is clear.
		editorView = m.merge.View()
	case m.pendingConflict.active:
		// Fix D (BUG2): the [S]/[D]/[M] guard is up — render the read-only
		// ours-vs-theirs PREVIEW (built by raiseConflictGuard) in place of the
		// main editor, so the diff is visible before the user chooses. Mutually
		// exclusive with the IsActive case above: pendingConflict is always
		// cleared before mergemode.Enter activates the resolver.
		editorView = m.merge.View()
	case m.pendingLoad.active:
		// Non-destructive anti-flash: render the editor's empty frame while a
		// load is in flight, leaving the real buffer intact (preserves 16138bd
		// without the SetContent("") stranding). RenderEmpty matches View()'s
		// height exactly, so the pane does not jump when the load lands.
		editorView = m.editor.RenderEmpty()
	}
	centerParts = append(centerParts, editorView)
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

	// §4: compose the error banner INTO body's existing top row rather than
	// prepending a new one — recalcLayout's contentH already reserves exactly
	// m.totalHeight-footer.Height() rows for body, and m.err is set/cleared
	// at sites that never call recalcLayout, so a row "reserved" there would
	// desync from a live m.err transition. Prepending unconditionally added a
	// row ABOVE that budget, which MaxHeight below then clipped off the
	// footer's bottom row to compensate — composing within the existing
	// budget here means the total height is always exactly m.totalHeight.
	body = overlayErrorLine(body, m.err, m.styles)

	clamp := lipgloss.NewStyle().MaxWidth(m.totalWidth).MaxHeight(m.totalHeight)
	frame := lipgloss.JoinVertical(lipgloss.Left, body, m.footer.View())
	return tea.NewView(clamp.Render(frame))
}

// overlayErrorLine composes err onto body's first rendered row (its top
// border row) in place, so no row is added and the frame's total height
// never exceeds its existing budget. A nil err leaves body unchanged.
//
// B1: lipgloss v2's Width(n) WORD-WRAPS content that doesn't fit rather than
// truncating it, so a long error (a long path is common) used to expand
// lines[0] into several physical rows — growing body past recalcLayout's
// contentH budget, which the outer MaxHeight(m.totalHeight) in View() then
// satisfied by silently clipping the footer's bottom row instead. MaxHeight(1)
// here constrains the rendered error to exactly the one row this function's
// own contract promises (verified: Width+MaxWidth+MaxHeight(1) always yields
// a single row of exactly rowWidth cells, never more).
//
// S2: err.Error() can carry a filename (from a real OS error) containing raw
// C0 control bytes (e.g. an embedded ESC), which — written verbatim into the
// terminal frame — would inject escape sequences. sanitizeErrorText strips
// C0 bytes before this ever reaches lipgloss.Render.
func overlayErrorLine(body string, err error, st styles.Styles) string {
	if err == nil {
		return body
	}
	lines := strings.Split(body, "\n")
	if len(lines) == 0 {
		return body
	}
	rowWidth := lipgloss.Width(lines[0])
	errLine := st.Error.Width(rowWidth).MaxWidth(rowWidth).MaxHeight(1).
		Render(" error: " + sanitizeErrorText(err.Error()))
	lines[0] = errLine
	return strings.Join(lines, "\n")
}

// sanitizeErrorText strips C0 control bytes (0x00-0x1F, including ESC) from
// display text at the terminal-frame boundary — an untrusted error string
// (e.g. built from a filename or path an external actor controls) must never
// be able to inject an escape sequence into the rendered frame (S2).
func sanitizeErrorText(s string) string {
	return strings.Map(func(r rune) rune {
		if r < 0x20 {
			return -1
		}
		return r
	}, s)
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
