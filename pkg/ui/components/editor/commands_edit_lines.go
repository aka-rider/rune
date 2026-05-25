package editor

import (
	"strings"

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

func execIndent(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	indentStr := "\t"
	c := all[0]

	if !c.HasSelection() {
		line := ctx.Buffer.OffsetToLineCol(c.Position).Line
		ls := ctx.Buffer.LineStart(line)
		e := buffer.Edit{Start: ls, End: ls, Insert: indentStr}
		newPos := c.Position + len(indentStr)
		newC := cursor.Cursor{Position: newPos, Anchor: newPos, ID: c.ID}
		sorted := buffer.CloneAndSortEditsDescending([]buffer.Edit{e})
		return command.Result{
			Operation: command.Operation{
				Kind:    command.OperationEditBuffer,
				Edits:   sorted,
				Cursors: cursor.NewCursorSetFrom([]cursor.Cursor{newC}),
			},
		}
	}

	startLine := ctx.Buffer.OffsetToLineCol(c.SelectionStart()).Line
	endLine := ctx.Buffer.OffsetToLineCol(c.SelectionEnd()).Line
	if ctx.Buffer.OffsetToLineCol(c.SelectionEnd()).Col == 0 && endLine > startLine {
		endLine--
	}

	var edits []buffer.Edit
	for line := endLine; line >= startLine; line-- {
		ls := ctx.Buffer.LineStart(line)
		edits = append(edits, buffer.Edit{Start: ls, End: ls, Insert: indentStr})
	}

	lineCount := endLine - startLine + 1
	newAnchor := c.Anchor + len(indentStr)
	newPos := c.Position + len(indentStr)*lineCount
	if c.Reversed() {
		newPos = c.Position + len(indentStr)
		newAnchor = c.Anchor + len(indentStr)*lineCount
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

func execOutdent(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	tabSize := 4
	c := all[0]

	startLine := ctx.Buffer.OffsetToLineCol(c.SelectionStart()).Line
	endLine := ctx.Buffer.OffsetToLineCol(c.SelectionEnd()).Line
	if !c.HasSelection() {
		endLine = startLine
	} else if ctx.Buffer.OffsetToLineCol(c.SelectionEnd()).Col == 0 && endLine > startLine {
		endLine--
	}

	var edits []buffer.Edit
	totalRemoved := 0
	lineRemovals := make(map[int]int)
	for line := endLine; line >= startLine; line-- {
		ls := ctx.Buffer.LineStart(line)
		lineContent := ctx.Buffer.Line(line)
		removed := 0
		if len(lineContent) > 0 && lineContent[0] == '\t' {
			removed = 1
		} else {
			for i := 0; i < tabSize && i < len(lineContent) && lineContent[i] == ' '; i++ {
				removed++
			}
		}
		if removed > 0 {
			edits = append(edits, buffer.Edit{Start: ls, End: ls + removed, Insert: ""})
			lineRemovals[line] = removed
			totalRemoved += removed
		}
	}

	if len(edits) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	// Compute shift for a given offset: sum of removals on lines before or at the offset
	shiftForOffset := func(offset int) int {
		bp := ctx.Buffer.OffsetToLineCol(offset)
		shift := 0
		for line := startLine; line <= endLine && line <= bp.Line; line++ {
			r := lineRemovals[line]
			if line < bp.Line {
				shift += r
			} else {
				// Same line: only shift if offset is past the removed chars
				ls := ctx.Buffer.LineStart(line)
				if offset >= ls+r {
					shift += r
				} else if offset > ls {
					shift += offset - ls
				}
			}
		}
		return shift
	}

	newPos := c.Position - shiftForOffset(c.Position)
	newAnchor := c.Anchor - shiftForOffset(c.Anchor)
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

func execToggleComment(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	c := all[0]
	startLine := ctx.Buffer.OffsetToLineCol(c.SelectionStart()).Line
	endLine := ctx.Buffer.OffsetToLineCol(c.SelectionEnd()).Line
	if !c.HasSelection() {
		endLine = startLine
	} else if ctx.Buffer.OffsetToLineCol(c.SelectionEnd()).Col == 0 && endLine > startLine {
		endLine--
	}

	allCommented := true
	for line := startLine; line <= endLine; line++ {
		content := ctx.Buffer.Line(line)
		trimmed := strings.TrimSpace(content)
		if trimmed == "" {
			continue
		}
		if !strings.HasPrefix(trimmed, "<!-- ") || !strings.HasSuffix(trimmed, " -->") {
			allCommented = false
			break
		}
	}

	var edits []buffer.Edit
	for line := endLine; line >= startLine; line-- {
		ls := ctx.Buffer.LineStart(line)
		content := ctx.Buffer.Line(line)
		if allCommented {
			trimmed := strings.TrimSpace(content)
			if trimmed == "" {
				continue
			}
			commentStart := strings.Index(content, "<!-- ")
			commentEnd := strings.LastIndex(content, " -->")
			if commentStart >= 0 && commentEnd >= 0 {
				edits = append(edits, buffer.Edit{
					Start:  ls + commentEnd,
					End:    ls + commentEnd + 4,
					Insert: "",
				})
				edits = append(edits, buffer.Edit{
					Start:  ls + commentStart,
					End:    ls + commentStart + 5,
					Insert: "",
				})
			}
		} else {
			indent := leadingWhitespaceRe.FindString(content)
			rest := content[len(indent):]
			edits = append(edits, buffer.Edit{
				Start:  ls,
				End:    ls + len(content),
				Insert: indent + "<!-- " + rest + " -->",
			})
		}
	}

	if len(edits) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	sorted := buffer.CloneAndSortEditsDescending(edits)

	// Compute cursor shift based on comment/uncomment on the cursor's line
	cursorLine := ctx.Buffer.OffsetToLineCol(c.Position).Line
	anchorLine := ctx.Buffer.OffsetToLineCol(c.Anchor).Line
	posShift := 0
	anchorShift := 0

	if allCommented {
		// Uncommenting: each line loses "<!-- " (5) before content and " -->" (4) after
		for line := startLine; line <= endLine; line++ {
			content := ctx.Buffer.Line(line)
			trimmed := strings.TrimSpace(content)
			if trimmed == "" {
				continue
			}
			commentStart := strings.Index(content, "<!-- ")
			if commentStart < 0 {
				continue
			}
			ls := ctx.Buffer.LineStart(line)
			removeStart := commentStart + 5 // length of "<!-- "
			if line == cursorLine && c.Position > ls+commentStart {
				if c.Position >= ls+removeStart {
					posShift -= 5
				} else {
					posShift -= (c.Position - (ls + commentStart))
				}
			}
			if line == anchorLine && c.Anchor > ls+commentStart {
				if c.Anchor >= ls+removeStart {
					anchorShift -= 5
				} else {
					anchorShift -= (c.Anchor - (ls + commentStart))
				}
			}
			// Also account for " -->" removal (4 chars) — only matters if cursor is past it
			commentEnd := strings.LastIndex(content, " -->")
			if line == cursorLine && c.Position > ls+commentEnd {
				posShift -= 4
			}
			if line == anchorLine && c.Anchor > ls+commentEnd {
				anchorShift -= 4
			}
		}
	} else {
		// Commenting: each line gains "<!-- " (5) before content and " -->" (4) after
		for line := startLine; line <= endLine; line++ {
			content := ctx.Buffer.Line(line)
			indent := leadingWhitespaceRe.FindString(content)
			ls := ctx.Buffer.LineStart(line)
			insertPoint := ls + len(indent)
			if line == cursorLine && c.Position >= insertPoint {
				posShift += 5
			}
			if line == anchorLine && c.Anchor >= insertPoint {
				anchorShift += 5
			}
		}
	}

	newPos := c.Position + posShift
	newAnchor := c.Anchor + anchorShift
	if newPos < 0 {
		newPos = 0
	}
	if newAnchor < 0 {
		newAnchor = 0
	}
	newC := cursor.Cursor{Position: newPos, Anchor: newAnchor, ID: c.ID}
	return command.Result{
		Operation: command.Operation{
			Kind:    command.OperationEditBuffer,
			Edits:   sorted,
			Cursors: cursor.NewCursorSetFrom([]cursor.Cursor{newC}),
		},
	}
}
