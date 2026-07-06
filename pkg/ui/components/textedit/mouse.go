package textedit

import (
	"rune/pkg/editor/coords"
	"rune/pkg/editor/cursor"
)

// ---- Low-level mouse helpers (D3) ----

// DisplayToBuffer converts a display-point to a buffer point and offset.
func (m Model) DisplayToBuffer(dp coords.DisplayPoint) (coords.BufferPoint, int) {
	wrapRow := dp.Row + m.viewport.TopRow
	wrapCol := dp.Col + m.viewport.ScrollCol
	if wrapRow < 0 {
		wrapRow = 0
	}
	if wrapRow >= m.wrapSnap.TotalRows {
		wrapRow = m.wrapSnap.TotalRows - 1
	}
	if wrapRow < 0 {
		return coords.BufferPoint{Line: 0, Col: 0}, 0
	}
	wp := coords.WrapPoint{Row: wrapRow, Col: wrapCol}
	sp := m.wrapSnap.WrapToSyntax(wp)
	bp := m.syntaxSnap.SyntaxToBuffer(sp)
	return bp, m.buf.LineColToOffset(bp)
}

// MousePositionCursor moves the primary cursor to the given buffer offset.
func (m Model) MousePositionCursor(offset int) Model {
	primary := cursor.Cursor{
		Position: offset,
		Anchor:   offset,
		ID:       1,
	}
	m.cursors = cursor.NewCursorSetFrom([]cursor.Cursor{primary})
	return m
}

// MouseExtendSelection extends the primary cursor's selection to the given offset.
func (m Model) MouseExtendSelection(offset int) Model {
	primary := m.cursors.Primary()
	primary.Position = offset
	m.cursors = cursor.NewCursorSetFrom([]cursor.Cursor{primary})
	return m
}

// MouseSelectWord selects the word at the given offset.
func (m Model) MouseSelectWord(offset int) Model {
	start := wordLeftOffset(m.buf, offset)
	end := wordRightOffset(m.buf, offset)
	primary := cursor.Cursor{
		Position: end,
		Anchor:   start,
		ID:       1,
	}
	m.cursors = cursor.NewCursorSetFrom([]cursor.Cursor{primary})
	return m
}

// MouseSelectLine selects the line containing the given offset.
func (m Model) MouseSelectLine(line int) Model {
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
	return m
}

// MouseAddCursor adds a new cursor at the given offset.
func (m Model) MouseAddCursor(offset int) Model {
	m.cursors = m.cursors.Add(cursor.Cursor{Position: offset, Anchor: offset})
	return m
}
