package markdownedit

import (
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/editor/coords"
	"rune/pkg/editor/display"
	"rune/pkg/ui/listnav"
)

const multiClickThreshold = 500 * time.Millisecond

// chebyshevDistance is the Chebyshev (king-move) distance between two cells —
// the max of the per-axis deltas, so it treats a diagonal move the same as
// an orthogonal one of the same reach. Used by handleMouseClick's multi-click
// tolerance.
func chebyshevDistance(x1, y1, x2, y2 int) int {
	dx := x1 - x2
	if dx < 0 {
		dx = -dx
	}
	dy := y1 - y2
	if dy < 0 {
		dy = -dy
	}
	if dx > dy {
		return dx
	}
	return dy
}

// mouseState holds the click-run/multi-click bookkeeping only. There is no
// drag state to reset on release: Bubble Tea v2.0.6 delivers
// tea.MouseReleaseMsg, but a drag is entirely derivable per-motion-event from
// the held button (handleMouseMotion checks msg.Button), so nothing needs to
// persist across events. A prior dragAnchor field was write-only (the real
// selection anchor lives in textedit) and a prior dragging bool was written
// but never read for behavior — both deleted (M1; the workspace divider drag
// is a natural future consumer of MouseReleaseMsg, not this component).
type mouseState struct {
	lastClickTime time.Time
	lastClickX    int
	lastClickY    int
	clickCount    int
}

func (m Model) handleMouseClick(msg tea.MouseClickMsg, now time.Time) (Model, tea.Cmd) {
	if !m.Model.Focused() {
		return m, nil
	}
	if msg.Button != tea.MouseLeft {
		return m, nil
	}

	g := m.Model.Geom()
	dp := coords.DisplayPoint{
		Row: msg.Y - g.OffsetY,
		Col: msg.X - g.OffsetX,
	}
	if dp.Row < 0 {
		return m, nil
	}
	if dp.Col < 0 {
		dp.Col = 0
	}

	// Skip clicks that land on image-reserved or table-border/separator rows.
	displayRow := dp.Row + g.Viewport.TopRow
	if displayRow >= 0 && displayRow < len(g.Snap.Lines) {
		l := g.Snap.Lines[displayRow]
		if l.ImagePath != "" || display.IsTableSeparatorRow(l) {
			return m, nil
		}
	}

	bp, offset := m.Model.DisplayToBuffer(dp)

	// Detect multi-click (link clicks participate too: a single click positions
	// the caret and reveals the link; a second click on it follows). Tolerance
	// is Chebyshev distance <= 1 cell, not pixel-exact — a human double-click
	// routinely lands a cell off between clicks (hand tremor, terminal cell
	// rounding), and demanding the exact same cell silently downgraded it to
	// two single clicks.
	clickCount := 1
	if now.Sub(m.mouse.lastClickTime) < multiClickThreshold &&
		chebyshevDistance(msg.X, msg.Y, m.mouse.lastClickX, m.mouse.lastClickY) <= 1 {
		clickCount = m.mouse.clickCount + 1
	}
	m.mouse.lastClickTime = now
	m.mouse.lastClickX = msg.X
	m.mouse.lastClickY = msg.Y
	m.mouse.clickCount = clickCount

	switch {
	case msg.Mod&tea.ModAlt != 0:
		m.Model = m.Model.MouseAddCursor(offset)
	case msg.Mod&tea.ModShift != 0:
		m.Model = m.Model.MouseExtendSelection(offset)
	case clickCount >= 3:
		m.Model = m.Model.MouseSelectLine(bp.Line)
	case clickCount == 2:
		// Double-click on a link follows it (Ctrl is an alias — not inspected);
		// double-click elsewhere selects the word. Following works even in a
		// read-only doc (it doesn't edit).
		if la, ok := m.linkAtLine(bp, offset); ok {
			m.Model = m.Model.MousePositionCursor(offset)
			return m, func() tea.Msg { return la }
		}
		m.Model = m.Model.MouseSelectWord(offset)
	default:
		m.Model = m.Model.MousePositionCursor(offset)
	}

	return m, nil
}

// LinkAtCursor returns the raw target of the navigable link under the primary
// caret (as written, e.g. "pages/foo.md" or "https://…") for the footer hint. It
// does NOT resolve / touch the filesystem — it runs on the cursor-move hot path
// (syncCursorToFooter, every keypress); resolution (and its os.Stat) happens only
// on a follow.
func (m Model) LinkAtCursor() (string, bool) {
	off := m.Model.CursorOffset()
	raw, _, ok := m.rawLinkAtLine(m.Model.OffsetToLineCol(off), off)
	return raw, ok
}

// rawLinkAtLine reports the navigable link target AS WRITTEN whose buffer range
// contains offset on the given line — no resolution, no I/O. appendMD is true for
// wiki links. Images, non-links, and empty/data: targets yield no hit.
func (m Model) rawLinkAtLine(bp coords.BufferPoint, offset int) (raw string, appendMD bool, ok bool) {
	g := m.Model.Geom()
	ss := g.Syntax
	if bp.Line < 0 || bp.Line >= len(ss.Lines) {
		return "", false, false
	}
	for _, sp := range ss.Lines[bp.Line].Spans {
		if sp.BufferStart > offset {
			break
		}
		if offset >= sp.BufferEnd {
			continue
		}
		if sp.LinkRole() != display.LinkRoleNavigable {
			continue // images and non-link spans are not navigable
		}
		raw, appendMD = sp.LinkURL, false
		if sp.Kind == display.TokenWikiLink {
			raw, appendMD = sp.WikiLinkTarget, true
		}
		raw = strings.TrimSpace(raw)
		if raw == "" || strings.HasPrefix(strings.ToLower(raw), "data:") {
			return "", false, false // not a navigable link
		}
		return raw, appendMD, true
	}
	return "", false, false
}

// linkAt reports the navigable link under a buffer byte offset, fully RESOLVED
// (does I/O). Used by the keyboard follow path, which knows only the caret offset.
func (m Model) linkAt(offset int) (LinkActivatedMsg, bool) {
	return m.linkAtLine(m.Model.OffsetToLineCol(offset), offset)
}

// linkAtLine resolves the navigable link under offset on the given line into a
// LinkActivatedMsg the workspace acts on by Kind — via the ONE resolver (resolveRef,
// existence-checked). A relative target that resolves to no existing file is
// LinkMissing (still a hit; the follow reports it). The follow path only.
func (m Model) linkAtLine(bp coords.BufferPoint, offset int) (LinkActivatedMsg, bool) {
	raw, appendMD, ok := m.rawLinkAtLine(bp, offset)
	if !ok {
		return LinkActivatedMsg{}, false
	}
	if isExternalURL(raw) {
		return LinkActivatedMsg{Raw: raw, Kind: LinkExternal, Dest: raw}, true
	}
	if abs, found := resolveRef(m.fs, raw, m.docDir(), m.root, appendMD); found {
		return LinkActivatedMsg{Raw: raw, Kind: LinkInternal, Dest: abs}, true
	}
	return LinkActivatedMsg{Raw: raw, Kind: LinkMissing}, true
}

func (m Model) handleMouseMotion(msg tea.MouseMotionMsg) (Model, tea.Cmd) {
	if !m.Model.Focused() {
		return m, nil
	}
	if msg.Button != tea.MouseLeft {
		return m, nil
	}

	g := m.Model.Geom()
	dp := coords.DisplayPoint{
		Row: msg.Y - g.OffsetY,
		Col: msg.X - g.OffsetX,
	}
	if dp.Row < 0 {
		dp.Row = 0
	}
	if dp.Col < 0 {
		dp.Col = 0
	}

	// Skip motion events landing on image-reserved or table-border/separator
	// rows — hold the selection at its last valid endpoint rather than
	// extending it through a decorative row.
	displayRow := dp.Row + g.Viewport.TopRow
	if displayRow >= 0 && displayRow < len(g.Snap.Lines) {
		l := g.Snap.Lines[displayRow]
		if l.ImagePath != "" || display.IsTableSeparatorRow(l) {
			return m, nil
		}
	}

	_, offset := m.Model.DisplayToBuffer(dp)

	// Extend selection from the click anchor to the current drag position.
	m.Model = m.Model.MouseExtendSelection(offset)
	return m, nil
}

func (m Model) handleMouseWheel(msg tea.MouseWheelMsg) (Model, tea.Cmd) {
	if !m.Model.Focused() {
		return m, nil
	}

	g := m.Model.Geom()
	vp := g.Viewport

	switch msg.Button {
	case tea.MouseWheelUp:
		vp.TopRow -= listnav.WheelLines
		if vp.TopRow < 0 {
			vp.TopRow = 0
		}
	case tea.MouseWheelDown:
		vp.TopRow += listnav.WheelLines
		maxTop := g.Snap.TotalRows - g.ContentHeight
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
	m, cmd = m.syncImageViewState()
	return m, cmd
}
