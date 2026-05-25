package editor

import (
	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
)

func lineRangeForCursor(buf buffer.Buffer, c cursor.Cursor) (startLine, endLine int) {
	if c.HasSelection() {
		startLine = buf.OffsetToLineCol(c.SelectionStart()).Line
		endLine = buf.OffsetToLineCol(c.SelectionEnd()).Line
		if buf.OffsetToLineCol(c.SelectionEnd()).Col == 0 && endLine > startLine {
			endLine--
		}
	} else {
		bp := buf.OffsetToLineCol(c.Position)
		startLine = bp.Line
		endLine = bp.Line
	}
	return
}

func execMoveLineUp(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}
	if len(all) > 1 {
		return execMoveLineUpMulti(ctx)
	}

	c := all[0]
	startLine, endLine := lineRangeForCursor(ctx.Buffer, c)

	if startLine == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	aboveLine := ctx.Buffer.Line(startLine - 1)
	aboveStart := ctx.Buffer.LineStart(startLine - 1)
	aboveEnd := ctx.Buffer.LineStart(startLine)

	var blockLines []string
	for line := startLine; line <= endLine; line++ {
		blockLines = append(blockLines, ctx.Buffer.Line(line))
	}

	blockEnd := lineEndWithNewline(ctx.Buffer, endLine)

	var edits []buffer.Edit
	edits = append(edits, buffer.Edit{
		Start:  aboveStart,
		End:    blockEnd,
		Insert: joinLines(blockLines) + "\n" + aboveLine + trailingNewline(ctx.Buffer, endLine),
	})

	shift := -(aboveEnd - aboveStart)
	newPos := c.Position + shift
	newAnchor := c.Anchor + shift
	if newPos < 0 {
		newPos = 0
	}
	if newAnchor < 0 {
		newAnchor = 0
	}
	newC := cursor.Cursor{Position: newPos, Anchor: newAnchor, ID: c.ID}

	sorted := buffer.CloneAndSortEditsDescending(edits)
	return command.Result{
		Operation: command.Operation{
			Kind:    command.OperationEditBuffer,
			Edits:   sorted,
			Cursors: cursor.NewCursorSetFrom([]cursor.Cursor{newC}),
		},
	}
}

func execMoveLineDown(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}
	if len(all) > 1 {
		return execMoveLineDownMulti(ctx)
	}

	c := all[0]
	startLine, endLine := lineRangeForCursor(ctx.Buffer, c)
	lineCount := ctx.Buffer.LineCount()

	if endLine >= lineCount-1 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	belowLine := ctx.Buffer.Line(endLine + 1)
	belowEnd := lineEndWithNewline(ctx.Buffer, endLine+1)

	var blockLines []string
	for line := startLine; line <= endLine; line++ {
		blockLines = append(blockLines, ctx.Buffer.Line(line))
	}

	blockStart := ctx.Buffer.LineStart(startLine)

	var edits []buffer.Edit
	edits = append(edits, buffer.Edit{
		Start:  blockStart,
		End:    belowEnd,
		Insert: belowLine + "\n" + joinLines(blockLines) + trailingNewline(ctx.Buffer, endLine+1),
	})

	shift := len(belowLine) + 1
	newPos := c.Position + shift
	newAnchor := c.Anchor + shift
	newC := cursor.Cursor{Position: newPos, Anchor: newAnchor, ID: c.ID}

	sorted := buffer.CloneAndSortEditsDescending(edits)
	return command.Result{
		Operation: command.Operation{
			Kind:    command.OperationEditBuffer,
			Edits:   sorted,
			Cursors: cursor.NewCursorSetFrom([]cursor.Cursor{newC}),
		},
	}
}

func execCloneLineUp(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}
	if len(all) > 1 {
		return execCloneLineUpMulti(ctx)
	}

	c := all[0]
	startLine, endLine := lineRangeForCursor(ctx.Buffer, c)

	var blockLines []string
	for line := startLine; line <= endLine; line++ {
		blockLines = append(blockLines, ctx.Buffer.Line(line))
	}

	insertAt := ctx.Buffer.LineStart(startLine)
	insertText := joinLines(blockLines) + "\n"

	edits := []buffer.Edit{{Start: insertAt, End: insertAt, Insert: insertText}}

	shift := len(insertText)
	newPos := c.Position + shift
	newAnchor := c.Anchor + shift
	newC := cursor.Cursor{Position: newPos, Anchor: newAnchor, ID: c.ID}

	sorted := buffer.CloneAndSortEditsDescending(edits)
	return command.Result{
		Operation: command.Operation{
			Kind:    command.OperationEditBuffer,
			Edits:   sorted,
			Cursors: cursor.NewCursorSetFrom([]cursor.Cursor{newC}),
		},
	}
}

func execCloneLineDown(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}
	if len(all) > 1 {
		return execCloneLineDownMulti(ctx)
	}

	c := all[0]
	startLine, endLine := lineRangeForCursor(ctx.Buffer, c)
	lineCount := ctx.Buffer.LineCount()

	var blockLines []string
	for line := startLine; line <= endLine; line++ {
		blockLines = append(blockLines, ctx.Buffer.Line(line))
	}

	var insertAt int
	var insertText string
	if endLine == lineCount-1 {
		insertAt = ctx.Buffer.LineEnd(endLine)
		insertText = "\n" + joinLines(blockLines)
	} else {
		insertAt = ctx.Buffer.LineStart(endLine + 1)
		insertText = joinLines(blockLines) + "\n"
	}

	edits := []buffer.Edit{{Start: insertAt, End: insertAt, Insert: insertText}}

	newC := cursor.Cursor{Position: c.Position, Anchor: c.Anchor, ID: c.ID}

	sorted := buffer.CloneAndSortEditsDescending(edits)
	return command.Result{
		Operation: command.Operation{
			Kind:    command.OperationEditBuffer,
			Edits:   sorted,
			Cursors: cursor.NewCursorSetFrom([]cursor.Cursor{newC}),
		},
	}
}

func joinLines(lines []string) string {
	if len(lines) == 0 {
		return ""
	}
	result := lines[0]
	for i := 1; i < len(lines); i++ {
		result += "\n" + lines[i]
	}
	return result
}

func lineEndWithNewline(buf buffer.Buffer, line int) int {
	lineCount := buf.LineCount()
	if line < lineCount-1 {
		return buf.LineStart(line + 1)
	}
	return buf.LineEnd(line)
}

func trailingNewline(buf buffer.Buffer, line int) string {
	if line < buf.LineCount()-1 {
		return "\n"
	}
	return ""
}

func registerLineEditCommands(builder command.Builder) (command.Builder, error) {
	var err error

	builder, err = builder.Register(command.Command{
		Name:    "edit.move-line-up",
		When:    "editorFocused && !readOnly",
		Execute: execMoveLineUp,
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name:    "edit.move-line-down",
		When:    "editorFocused && !readOnly",
		Execute: execMoveLineDown,
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name:    "edit.clone-line-up",
		When:    "editorFocused && !readOnly",
		Execute: execCloneLineUp,
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name:    "edit.clone-line-down",
		When:    "editorFocused && !readOnly",
		Execute: execCloneLineDown,
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name:    "edit.indent",
		When:    "editorFocused && !readOnly",
		Execute: execIndent,
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name:    "edit.outdent",
		When:    "editorFocused && !readOnly",
		Execute: execOutdent,
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name:    "edit.toggle-comment",
		When:    "editorFocused && !readOnly",
		Execute: execToggleComment,
	})
	if err != nil {
		return builder, err
	}

	return builder, nil
}
