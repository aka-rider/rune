package editor

import (
	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
)

func execMoveLineUpMulti(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}
	if len(all) == 1 {
		return execMoveLineUp(ctx)
	}

	ranges := unifyLineRanges(ctx.Buffer, all)

	if ranges[0].startLine == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	var edits []buffer.Edit
	for _, r := range ranges {
		aboveLine := ctx.Buffer.Line(r.startLine - 1)
		aboveStart := ctx.Buffer.LineStart(r.startLine - 1)

		var blockLines []string
		for line := r.startLine; line <= r.endLine; line++ {
			blockLines = append(blockLines, ctx.Buffer.Line(line))
		}

		blockEnd := lineEndWithNewline(ctx.Buffer, r.endLine)
		insertText := joinLines(blockLines) + "\n" + aboveLine + trailingNewline(ctx.Buffer, r.endLine)
		edits = append(edits, buffer.Edit{Start: aboveStart, End: blockEnd, Insert: insertText})
	}

	sorted := buffer.CloneAndSortEditsDescending(edits)

	var newCursors []cursor.Cursor
	for _, c := range all {
		bp := ctx.Buffer.OffsetToLineCol(c.Position)
		for _, r := range ranges {
			if bp.Line >= r.startLine && bp.Line <= r.endLine {
				aboveStart := ctx.Buffer.LineStart(r.startLine - 1)
				blockStart := ctx.Buffer.LineStart(r.startLine)
				shift := -(blockStart - aboveStart)
				newPos := c.Position + shift
				newAnchor := c.Anchor + shift
				if newPos < 0 {
					newPos = 0
				}
				if newAnchor < 0 {
					newAnchor = 0
				}
				newCursors = append(newCursors, cursor.Cursor{
					Position:   newPos,
					Anchor:     newAnchor,
					DesiredCol: c.DesiredCol,
					ID:         c.ID,
				})
				break
			}
		}
	}

	return command.Result{
		Operation: command.Operation{
			Kind:    command.OperationEditBuffer,
			Edits:   sorted,
			Cursors: cursor.NewCursorSetFrom(newCursors),
		},
	}
}

func execMoveLineDownMulti(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}
	if len(all) == 1 {
		return execMoveLineDown(ctx)
	}

	ranges := unifyLineRanges(ctx.Buffer, all)
	lineCount := ctx.Buffer.LineCount()

	if ranges[len(ranges)-1].endLine >= lineCount-1 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	var edits []buffer.Edit
	for i := len(ranges) - 1; i >= 0; i-- {
		r := ranges[i]
		belowLine := ctx.Buffer.Line(r.endLine + 1)
		belowEnd := lineEndWithNewline(ctx.Buffer, r.endLine+1)
		blockStart := ctx.Buffer.LineStart(r.startLine)

		var blockLines []string
		for line := r.startLine; line <= r.endLine; line++ {
			blockLines = append(blockLines, ctx.Buffer.Line(line))
		}

		insertText := belowLine + "\n" + joinLines(blockLines) + trailingNewline(ctx.Buffer, r.endLine+1)
		edits = append(edits, buffer.Edit{Start: blockStart, End: belowEnd, Insert: insertText})
	}

	sorted := buffer.CloneAndSortEditsDescending(edits)

	var newCursors []cursor.Cursor
	for _, c := range all {
		bp := ctx.Buffer.OffsetToLineCol(c.Position)
		for _, r := range ranges {
			if bp.Line >= r.startLine && bp.Line <= r.endLine {
				belowLine := ctx.Buffer.Line(r.endLine + 1)
				shift := len(belowLine) + 1
				newCursors = append(newCursors, cursor.Cursor{
					Position:   c.Position + shift,
					Anchor:     c.Anchor + shift,
					DesiredCol: c.DesiredCol,
					ID:         c.ID,
				})
				break
			}
		}
	}

	return command.Result{
		Operation: command.Operation{
			Kind:    command.OperationEditBuffer,
			Edits:   sorted,
			Cursors: cursor.NewCursorSetFrom(newCursors),
		},
	}
}

func execCloneLineUpMulti(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}
	if len(all) == 1 {
		return execCloneLineUp(ctx)
	}

	ranges := unifyLineRanges(ctx.Buffer, all)

	type rangeInsert struct {
		insertAt  int
		insertLen int
	}
	var insertions []rangeInsert
	var edits []buffer.Edit

	for _, r := range ranges {
		var blockLines []string
		for line := r.startLine; line <= r.endLine; line++ {
			blockLines = append(blockLines, ctx.Buffer.Line(line))
		}
		insertAt := ctx.Buffer.LineStart(r.startLine)
		insertText := joinLines(blockLines) + "\n"
		edits = append(edits, buffer.Edit{Start: insertAt, End: insertAt, Insert: insertText})
		insertions = append(insertions, rangeInsert{insertAt: insertAt, insertLen: len(insertText)})
	}

	sorted := buffer.CloneAndSortEditsDescending(edits)

	var newCursors []cursor.Cursor
	for _, c := range all {
		shift := 0
		for _, ins := range insertions {
			if ins.insertAt <= c.Position {
				shift += ins.insertLen
			}
		}
		newCursors = append(newCursors, cursor.Cursor{
			Position:   c.Position + shift,
			Anchor:     c.Anchor + shift,
			DesiredCol: c.DesiredCol,
			ID:         c.ID,
		})
	}

	return command.Result{
		Operation: command.Operation{
			Kind:    command.OperationEditBuffer,
			Edits:   sorted,
			Cursors: cursor.NewCursorSetFrom(newCursors),
		},
	}
}

func execCloneLineDownMulti(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}
	if len(all) == 1 {
		return execCloneLineDown(ctx)
	}

	ranges := unifyLineRanges(ctx.Buffer, all)
	lineCount := ctx.Buffer.LineCount()

	var edits []buffer.Edit
	for i := len(ranges) - 1; i >= 0; i-- {
		r := ranges[i]
		var blockLines []string
		for line := r.startLine; line <= r.endLine; line++ {
			blockLines = append(blockLines, ctx.Buffer.Line(line))
		}

		var insertAt int
		var insertText string
		if r.endLine == lineCount-1 {
			insertAt = ctx.Buffer.LineEnd(r.endLine)
			insertText = "\n" + joinLines(blockLines)
		} else {
			insertAt = ctx.Buffer.LineStart(r.endLine + 1)
			insertText = joinLines(blockLines) + "\n"
		}
		edits = append(edits, buffer.Edit{Start: insertAt, End: insertAt, Insert: insertText})
	}

	sorted := buffer.CloneAndSortEditsDescending(edits)

	var newCursors []cursor.Cursor
	for _, c := range all {
		newCursors = append(newCursors, cursor.Cursor{
			Position:   c.Position,
			Anchor:     c.Anchor,
			DesiredCol: c.DesiredCol,
			ID:         c.ID,
		})
	}

	return command.Result{
		Operation: command.Operation{
			Kind:    command.OperationEditBuffer,
			Edits:   sorted,
			Cursors: cursor.NewCursorSetFrom(newCursors),
		},
	}
}

func execDeleteLineMulti(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}
	if len(all) == 1 {
		return execDeleteLine(ctx)
	}

	ranges := unifyLineRanges(ctx.Buffer, all)
	lineCount := ctx.Buffer.LineCount()

	var edits []buffer.Edit
	for i := len(ranges) - 1; i >= 0; i-- {
		r := ranges[i]
		var e buffer.Edit
		if r.startLine == 0 && r.endLine >= lineCount-1 {
			e = buffer.Edit{Start: 0, End: ctx.Buffer.Len(), Insert: ""}
		} else if r.endLine < lineCount-1 {
			e = buffer.Edit{
				Start:  ctx.Buffer.LineStart(r.startLine),
				End:    ctx.Buffer.LineStart(r.endLine + 1),
				Insert: "",
			}
		} else {
			start := ctx.Buffer.LineStart(r.startLine)
			if r.startLine > 0 {
				start = ctx.Buffer.LineEnd(r.startLine - 1)
			}
			e = buffer.Edit{Start: start, End: ctx.Buffer.Len(), Insert: ""}
		}
		edits = append(edits, e)
	}

	sorted := buffer.CloneAndSortEditsDescending(edits)

	var newCursors []cursor.Cursor
	shift := 0
	for _, r := range ranges {
		pos := ctx.Buffer.LineStart(r.startLine) + shift
		if pos < 0 {
			pos = 0
		}
		newCursors = append(newCursors, cursor.Cursor{
			Position: pos,
			Anchor:   pos,
			ID:       r.cursorIDs[0],
		})
		var deleteLen int
		if r.startLine == 0 && r.endLine >= lineCount-1 {
			deleteLen = ctx.Buffer.Len()
		} else if r.endLine < lineCount-1 {
			deleteLen = ctx.Buffer.LineStart(r.endLine+1) - ctx.Buffer.LineStart(r.startLine)
		} else {
			start := ctx.Buffer.LineStart(r.startLine)
			if r.startLine > 0 {
				start = ctx.Buffer.LineEnd(r.startLine - 1)
			}
			deleteLen = ctx.Buffer.Len() - start
		}
		shift -= deleteLen
	}

	for i := range newCursors {
		if newCursors[i].Position < 0 {
			newCursors[i].Position = 0
		}
		if newCursors[i].Anchor < 0 {
			newCursors[i].Anchor = 0
		}
	}

	return command.Result{
		Operation: command.Operation{
			Kind:    command.OperationEditBuffer,
			Edits:   sorted,
			Cursors: cursor.NewCursorSetFrom(newCursors),
		},
	}
}
