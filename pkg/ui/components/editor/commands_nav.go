package editor

import (
	"unicode/utf8"

	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/coords"
	"rune/pkg/editor/cursor"
)

type charClass int

const (
	classWhitespace charClass = iota
	classWord
	classOther
)

func getClass(r rune) charClass {
	if r == ' ' || r == '\t' || r == '\n' || r == '\r' {
		return classWhitespace
	}
	if (r >= 'a' && r <= 'z') || (r >= 'A' && r <= 'Z') || (r >= '0' && r <= '9') || r == '_' {
		return classWord
	}
	return classOther
}

func prevRuneOffset(b buffer.Buffer, offset int) int {
	if offset <= 0 {
		return 0
	}
	start := offset - utf8.UTFMax
	if start < 0 {
		start = 0
	}
	s := b.Slice(start, offset)
	r, size := utf8.DecodeLastRuneInString(s)
	if r == utf8.RuneError && size <= 1 {
		return offset - 1
	}
	return offset - size
}

func nextRuneOffset(b buffer.Buffer, offset int) int {
	if offset >= b.Len() {
		return b.Len()
	}
	_, size := b.RuneAt(offset)
	if size == 0 {
		return offset + 1
	}
	return offset + size
}

func wordLeftOffset(b buffer.Buffer, offset int) int {
	if offset <= 0 {
		return 0
	}
	offset = prevRuneOffset(b, offset)
	r, _ := b.RuneAt(offset)
	startClass := getClass(r)

	if startClass == classWhitespace {
		for offset > 0 {
			prev := prevRuneOffset(b, offset)
			r, _ := b.RuneAt(prev)
			if getClass(r) != classWhitespace {
				break
			}
			offset = prev
		}
		if offset == 0 {
			return 0
		}
		offset = prevRuneOffset(b, offset)
		r, _ := b.RuneAt(offset)
		startClass = getClass(r)
	}

	for offset > 0 {
		prev := prevRuneOffset(b, offset)
		r, _ := b.RuneAt(prev)
		if getClass(r) != startClass {
			break
		}
		offset = prev
	}
	return offset
}

func wordRightOffset(b buffer.Buffer, offset int) int {
	if offset >= b.Len() {
		return offset
	}
	r, _ := b.RuneAt(offset)
	startClass := getClass(r)

	for offset < b.Len() {
		r, size := b.RuneAt(offset)
		if getClass(r) != startClass {
			break
		}
		offset += size
	}

	if startClass == classWhitespace {
		if offset < b.Len() {
			r, _ := b.RuneAt(offset)
			nextClass := getClass(r)
			for offset < b.Len() {
				r, size := b.RuneAt(offset)
				if getClass(r) != nextClass {
					break
				}
				offset += size
			}
		}
	}
	return offset
}

func lineStartOffset(b buffer.Buffer, offset int) int {
	bp := b.OffsetToLineCol(offset)
	lineStart := b.LineColToOffset(coords.BufferPoint{Line: bp.Line, Col: 0})

	firstNonWS := lineStart
	for firstNonWS < b.Len() {
		r, size := b.RuneAt(firstNonWS)
		if r == '\n' || (r != ' ' && r != '\t') {
			break
		}
		firstNonWS += size
	}

	if offset == firstNonWS {
		return lineStart
	}
	return firstNonWS
}

func lineEndOffset(b buffer.Buffer, offset int) int {
	bp := b.OffsetToLineCol(offset)
	end := b.LineColToOffset(coords.BufferPoint{Line: bp.Line, Col: 0})
	for end < b.Len() {
		r, size := b.RuneAt(end)
		if r == '\n' {
			break
		}
		end += size
	}
	return end
}

func updateHorizontal(ctx command.CommandContext, c cursor.Cursor, offset int, selectMode bool) cursor.Cursor {
	newC := cursor.Cursor{
		Position: offset,
		Anchor:   c.Anchor,
		ID:       c.ID,
	}

	if ctx.BufferToSyntax != nil {
		bp := ctx.Buffer.OffsetToLineCol(offset)
		sp := ctx.BufferToSyntax(bp)
		newC.DesiredCol = sp.Col
	} else {
		bp := ctx.Buffer.OffsetToLineCol(offset)
		newC.DesiredCol = bp.Col
	}

	if !selectMode {
		newC.Anchor = offset
	}
	return newC
}

func moveRow(ctx command.CommandContext, c cursor.Cursor, delta int, selectMode bool) cursor.Cursor {
	if ctx.BufferToSyntax == nil || ctx.SyntaxToWrap == nil || ctx.WrapToSyntax == nil || ctx.SyntaxToBuffer == nil {
		return c
	}
	bp := ctx.Buffer.OffsetToLineCol(c.Position)
	sp := ctx.BufferToSyntax(bp)
	sp.Col = c.DesiredCol

	wp := ctx.SyntaxToWrap(sp)
	wp.Row += delta

	if wp.Row < 0 {
		wp.Row = 0
		wp.Col = 0
	}
	if ctx.TotalRows != nil {
		if total := ctx.TotalRows(); total > 0 && wp.Row >= total {
			wp.Row = total - 1
			wp.Col = 999999
		}
	}

	sp2 := ctx.WrapToSyntax(wp)
	bp2 := ctx.SyntaxToBuffer(sp2)
	offset2 := ctx.Buffer.LineColToOffset(bp2)

	newC := cursor.Cursor{
		Position:   offset2,
		Anchor:     c.Anchor,
		DesiredCol: c.DesiredCol,
		ID:         c.ID,
	}
	if !selectMode {
		newC.Anchor = offset2
	}
	return newC
}

func handleCursorCmd(ctx command.CommandContext, selectMode bool, step func(c cursor.Cursor, selectMode bool) cursor.Cursor) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationMoveCursors, Cursors: ctx.Cursors}}
	}
	var newCursors []cursor.Cursor
	for _, c := range all {
		newCursors = append(newCursors, step(c, selectMode))
	}
	return command.Result{
		Operation: command.Operation{
			Kind:    command.OperationMoveCursors,
			Cursors: cursor.NewCursorSetFrom(newCursors),
		},
	}
}

func handleScrollCmd(ctx command.CommandContext, dx, dy int) command.Result {
	return command.Result{
		Operation: command.Operation{
			Kind:     command.OperationScroll,
			ScrollDX: dx,
			ScrollDY: dy,
		},
	}
}

func handleLeftCmd(ctx command.CommandContext, c cursor.Cursor, selectMode bool, stepFunc func(buffer.Buffer, int) int) cursor.Cursor {
	offset := stepFunc(ctx.Buffer, c.Position)
	if !selectMode && c.HasSelection() {
		offset = c.SelectionStart()
	}
	return updateHorizontal(ctx, c, offset, selectMode)
}

func handleRightCmd(ctx command.CommandContext, c cursor.Cursor, selectMode bool, stepFunc func(buffer.Buffer, int) int) cursor.Cursor {
	offset := stepFunc(ctx.Buffer, c.Position)
	if !selectMode && c.HasSelection() {
		offset = c.SelectionEnd()
	}
	return updateHorizontal(ctx, c, offset, selectMode)
}

func handleMoveToConst(ctx command.CommandContext, c cursor.Cursor, selectMode bool, offset int) cursor.Cursor {
	return updateHorizontal(ctx, c, offset, selectMode)
}

func handleMoveTo(ctx command.CommandContext, c cursor.Cursor, selectMode bool, stepFunc func(buffer.Buffer, int) int) cursor.Cursor {

	offset := stepFunc(ctx.Buffer, c.Position)
	if !selectMode && c.HasSelection() {
		if c.Reversed() {
			// Wait, does Home collapse to pos? Spec said: "navigation collapses to SelectionEnd/SelectionStart respectively... Exception left/right"
			// Let's just use c.Position as starting point for move if it collapses to position! No, spec says:
			// "navigation collapses to SelectionEnd ... Backward selection collapses to SelectionStart"
			// This is exactly `collapse to c.Position()` before applying move!
		}
		// It's already handled because we pass c.Position into stepFunc! (and updateHorizontal will turn off selection anchor since !selectMode)
	}
	return updateHorizontal(ctx, c, offset, selectMode)
}
