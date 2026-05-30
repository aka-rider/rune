package editor

import (
	"path/filepath"
	"strings"
	"time"
	"unicode/utf8"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/coords"
	"rune/pkg/editor/cursor"
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
	dragAnchor    int // byte offset where drag started
}

func displayToBuffer(dp coords.DisplayPoint, vp ViewportState, ws display.WrapSnapshot, ss display.SyntaxSnapshot) coords.BufferPoint {
	// Display → Wrap: account for viewport offset
	wrapRow := dp.Row + vp.TopRow
	wrapCol := dp.Col + vp.ScrollCol

	// Clamp row to valid range
	if wrapRow < 0 {
		wrapRow = 0
	}
	if wrapRow >= ws.TotalRows {
		wrapRow = ws.TotalRows - 1
	}
	if wrapRow < 0 {
		return coords.BufferPoint{Line: 0, Col: 0}
	}

	// Wrap → Syntax
	wp := coords.WrapPoint{Row: wrapRow, Col: wrapCol}
	sp := ws.WrapToSyntax(wp)

	// Syntax → Buffer
	bp := ss.SyntaxToBuffer(sp)
	return bp
}

func (m Model) handleMouseClick(msg tea.MouseClickMsg, now time.Time) (Model, tea.Cmd) {
	if !m.focused {
		return m, nil
	}
	if msg.Button != tea.MouseLeft {
		return m, nil
	}

	dp := coords.DisplayPoint{
		Row: msg.Y - m.offsetY - m.breadcrumb.Height(),
		Col: msg.X - m.offsetX,
	}
	if dp.Row < 0 {
		return m, nil
	}
	if dp.Col < 0 {
		dp.Col = 0
	}

	// Skip clicks that land on image-reserved display rows. The wrapSnap
	// doesn't account for image row expansion, so displayToBuffer would map
	// these rows to wrong buffer positions and potentially trigger reveal.
	if dp.Row+m.viewport.TopRow >= 0 && dp.Row+m.viewport.TopRow < len(m.snapshot.Lines) {
		if m.snapshot.Lines[dp.Row+m.viewport.TopRow].ImagePath != "" {
			return m, nil
		}
	}

	bp := displayToBuffer(dp, m.viewport, m.wrapSnap, m.syntaxSnap)
	offset := m.buf.LineColToOffset(bp)

	// Check if click is on a link span (wiki link or markdown link)
	if linkPath := m.resolveLinkClick(bp); linkPath != "" {
		// Position cursor at click for visual feedback
		m = m.mousePositionCursor(offset)
		m.mouse.dragAnchor = offset
		// Emit link click command
		return m, func() tea.Msg {
			return LinkClickedMsg{Path: linkPath}
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
		// Alt+click: add cursor
		m = m.mouseAddCursor(offset)
	case msg.Mod&tea.ModShift != 0:
		// Shift+click: extend selection
		m = m.mouseExtendSelection(offset)
	case clickCount >= 3:
		// Triple-click: select line
		m = m.mouseSelectLine(bp.Line)
	case clickCount == 2:
		// Double-click: select word
		m = m.mouseSelectWord(offset)
	default:
		// Single click: position cursor, clear selection and secondary cursors
		m = m.mousePositionCursor(offset)
		m.mouse.dragAnchor = offset
	}

	return m, nil
}

// resolveLinkClick checks if the given buffer point falls on a wiki link or
// markdown link span. Returns the resolved target path, or empty string if
// not on a link.
func (m Model) resolveLinkClick(bp coords.BufferPoint) string {
	if bp.Line < 0 || bp.Line >= len(m.syntaxSnap.Lines) {
		return ""
	}

	line := m.syntaxSnap.Lines[bp.Line]
	col := bp.Col

	for _, sp := range line.Spans {
		if sp.BufferStart > col {
			break
		}
		if col < sp.BufferStart {
			continue
		}
		if col >= sp.BufferEnd {
			continue
		}

		// Check if this span is a link
		switch sp.Kind {
		case display.TokenWikiLink:
			// Wiki link images are embedded content, not navigable links.
			if sp.WikiLinkIsImage {
				return ""
			}
			// Resolve wiki link target
			if sp.WikiLinkTarget == "" {
				return ""
			}
			return m.resolveWikiLinkTarget(sp.WikiLinkTarget)

		case display.TokenLink:
			// Markdown link — check if it's a file or URL
			if sp.ImagePath != "" {
				// Image path could be a file path
				lower := strings.ToLower(sp.ImagePath)
				if strings.HasPrefix(lower, "http://") || strings.HasPrefix(lower, "https://") || strings.HasPrefix(lower, "data:") {
					return "" // external URL — terminal handles it
				}
				// File path — resolve relative to file directory
				if filepath.IsAbs(sp.ImagePath) {
					return filepath.Clean(sp.ImagePath)
				}
				if m.filePath != "" {
					return filepath.Join(filepath.Dir(m.filePath), sp.ImagePath)
				}
			}
		}
	}
	return ""
}

func (m Model) handleMouseMotion(msg tea.MouseMotionMsg) (Model, tea.Cmd) {
	if !m.focused {
		return m, nil
	}
	if msg.Button != tea.MouseLeft {
		return m, nil
	}

	dp := coords.DisplayPoint{
		Row: msg.Y - m.offsetY - m.breadcrumb.Height(),
		Col: msg.X - m.offsetX,
	}
	if dp.Row < 0 {
		dp.Row = 0
	}
	if dp.Col < 0 {
		dp.Col = 0
	}

	bp := displayToBuffer(dp, m.viewport, m.wrapSnap, m.syntaxSnap)
	offset := m.buf.LineColToOffset(bp)

	if !m.mouse.dragging {
		m.mouse.dragging = true
	}

	// Extend selection from drag anchor to current position
	primary := cursor.Cursor{
		Position: offset,
		Anchor:   m.mouse.dragAnchor,
		ID:       1,
	}
	m.cursors = cursor.NewCursorSetFrom([]cursor.Cursor{primary})
	return m, nil
}

func (m Model) handleMouseWheel(msg tea.MouseWheelMsg) (Model, tea.Cmd) {
	if !m.focused {
		return m, nil
	}

	switch msg.Button {
	case tea.MouseWheelUp:
		m.viewport.TopRow -= mouseScrollLines
		if m.viewport.TopRow < 0 {
			m.viewport.TopRow = 0
		}
	case tea.MouseWheelDown:
		m.viewport.TopRow += mouseScrollLines
		maxTop := m.snapshot.TotalRows - m.contentHeight()
		if maxTop < 0 {
			maxTop = 0
		}
		if m.viewport.TopRow > maxTop {
			m.viewport.TopRow = maxTop
		}
	}
	return m, nil
}

func (m Model) mousePositionCursor(offset int) Model {
	primary := cursor.Cursor{
		Position: offset,
		Anchor:   offset,
		ID:       1,
	}
	m.cursors = cursor.NewCursorSetFrom([]cursor.Cursor{primary})
	return m
}

func (m Model) mouseExtendSelection(offset int) Model {
	primary := m.cursors.Primary()
	primary.Position = offset
	m.cursors = cursor.NewCursorSetFrom([]cursor.Cursor{primary})
	return m
}

func (m Model) mouseAddCursor(offset int) Model {
	newCursor := cursor.Cursor{
		Position: offset,
		Anchor:   offset,
	}
	m.cursors = m.cursors.Add(newCursor)
	return m
}

func (m Model) mouseSelectWord(offset int) Model {
	start := wordBoundaryLeft(m.buf, offset)
	end := wordBoundaryRight(m.buf, offset)
	primary := cursor.Cursor{
		Position: end,
		Anchor:   start,
		ID:       1,
	}
	m.cursors = cursor.NewCursorSetFrom([]cursor.Cursor{primary})
	m.mouse.dragAnchor = start
	return m
}

func (m Model) mouseSelectLine(line int) Model {
	lineStart := m.buf.LineStart(line)
	var lineEnd int
	if line >= m.buf.LineCount()-1 {
		lineEnd = m.buf.Len()
	} else {
		lineEnd = m.buf.LineStart(line + 1)
	}
	primary := cursor.Cursor{
		Position: lineEnd,
		Anchor:   lineStart,
		ID:       1,
	}
	m.cursors = cursor.NewCursorSetFrom([]cursor.Cursor{primary})
	m.mouse.dragAnchor = lineStart
	return m
}

// wordBoundaryLeft finds the start of the word at offset.
func wordBoundaryLeft(b buffer.Buffer, offset int) int {
	if offset <= 0 {
		return 0
	}
	if offset > b.Len() {
		offset = b.Len()
	}

	// Determine class from the character at offset (what was clicked on).
	// If offset == b.Len(), step back one rune to get the last character.
	checkOff := offset
	if checkOff >= b.Len() {
		checkOff = prevRuneOffsetBuf(b, checkOff)
	}
	r, _ := b.RuneAt(checkOff)
	cls := getClass(r)

	if cls == classWhitespace {
		// Move left past whitespace, then find word start
		pos := checkOff
		for pos > 0 {
			prev := prevRuneOffsetBuf(b, pos)
			pr, _ := b.RuneAt(prev)
			if getClass(pr) != classWhitespace {
				pos = prev
				cls = getClass(pr)
				break
			}
			pos = prev
		}
		if cls == classWhitespace {
			return 0
		}
		checkOff = pos
	}

	// Scan left while same class
	pos := checkOff
	for pos > 0 {
		prev := prevRuneOffsetBuf(b, pos)
		pr, _ := b.RuneAt(prev)
		if getClass(pr) != cls {
			break
		}
		pos = prev
	}
	return pos
}

// wordBoundaryRight finds the end of the word at offset.
func wordBoundaryRight(b buffer.Buffer, offset int) int {
	if offset >= b.Len() {
		return b.Len()
	}
	r, _ := b.RuneAt(offset)
	cls := getClass(r)
	if cls == classWhitespace {
		// Move right past whitespace, then find word end
		pos := offset
		for pos < b.Len() {
			pr, size := b.RuneAt(pos)
			if getClass(pr) != classWhitespace {
				cls = getClass(pr)
				offset = pos
				break
			}
			pos += size
		}
		if cls == classWhitespace {
			return b.Len()
		}
	}
	pos := offset
	for pos < b.Len() {
		pr, size := b.RuneAt(pos)
		if getClass(pr) != cls {
			break
		}
		pos += size
	}
	return pos
}

func prevRuneOffsetBuf(b buffer.Buffer, offset int) int {
	if offset <= 0 {
		return 0
	}
	start := offset - utf8.UTFMax
	if start < 0 {
		start = 0
	}
	s := b.Slice(start, offset)
	_, size := utf8.DecodeLastRuneInString(s)
	if size == 0 {
		return offset - 1
	}
	return offset - size
}

func registerMouseCommands(builder command.Builder) (command.Builder, error) {
	return builder, nil
}
