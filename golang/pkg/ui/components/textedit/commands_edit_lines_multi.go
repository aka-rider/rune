package textedit

import (
	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
)

// execDeleteLineMulti deletes the line under each cursor (deduped so two
// cursors on the same line delete it once). Registered as edit.delete-line —
// the single-cursor case used to be a near-verbatim duplicate of this body
// under its own name (execDeleteLine); perLineEdits' dedupe already handles
// the single-cursor case identically, so that duplicate is gone.
func execDeleteLineMulti(ctx command.CommandContext) command.Result {
	return perLineEdits(ctx, true, func(line int, c cursor.Cursor) (buffer.Edit, bool) {
		lineCount := ctx.Buffer.LineCount()
		if lineCount == 1 {
			return buffer.Edit{Start: 0, End: ctx.Buffer.Len(), Insert: ""}, true
		}
		if line < lineCount-1 {
			return buffer.Edit{Start: ctx.Buffer.LineStart(line), End: ctx.Buffer.LineStart(line + 1), Insert: ""}, true
		}
		return buffer.Edit{Start: ctx.Buffer.LineEnd(line - 1), End: ctx.Buffer.LineEnd(line), Insert: ""}, true
	})
}

func execCloneLineUp(ctx command.CommandContext) command.Result {
	return perLineEdits(ctx, false, func(line int, c cursor.Cursor) (buffer.Edit, bool) {
		if line == 0 {
			return buffer.Edit{}, false
		}
		lineStart := ctx.Buffer.LineStart(line)
		lineEnd := ctx.Buffer.LineEnd(line)
		lineText := ctx.Buffer.Slice(lineStart, lineEnd)
		return buffer.Edit{Start: lineStart, End: lineStart, Insert: lineText + "\n"}, true
	})
}

func execCloneLineDown(ctx command.CommandContext) command.Result {
	return perLineEdits(ctx, false, func(line int, c cursor.Cursor) (buffer.Edit, bool) {
		lineStart := ctx.Buffer.LineStart(line)
		lineEnd := ctx.Buffer.LineEnd(line)
		return buffer.Edit{Start: lineEnd, End: lineEnd, Insert: "\n" + ctx.Buffer.Slice(lineStart, lineEnd)}, true
	})
}

func execMoveLineUp(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return noneResult()
	}
	c := all[0]
	bp := ctx.Buffer.OffsetToLineCol(c.Position)
	if bp.Line == 0 {
		return noneResult()
	}
	L := bp.Line
	prevStart := ctx.Buffer.LineStart(L - 1)
	prevEnd := ctx.Buffer.LineEnd(L - 1)
	lineStart := ctx.Buffer.LineStart(L)
	lineEnd := ctx.Buffer.LineEnd(L)
	textPrev := ctx.Buffer.Slice(prevStart, prevEnd)
	textL := ctx.Buffer.Slice(lineStart, lineEnd)
	e := buffer.Edit{Start: prevStart, End: lineEnd, Insert: textL + "\n" + textPrev}
	col := c.Position - lineStart
	if col < 0 {
		col = 0
	} else if col > len(textL) {
		col = len(textL)
	}
	newPos := prevStart + col
	newC := cursor.Cursor{Position: newPos, Anchor: newPos, ID: c.ID, DesiredCol: c.DesiredCol}
	return command.Result{
		Operation: command.Operation{
			Kind:    command.OperationEditBuffer,
			Edits:   []buffer.Edit{e},
			Cursors: cursor.NewCursorSetFrom([]cursor.Cursor{newC}),
		},
	}
}

func execMoveLineDown(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return noneResult()
	}
	c := all[0]
	bp := ctx.Buffer.OffsetToLineCol(c.Position)
	lineCount := ctx.Buffer.LineCount()
	if bp.Line >= lineCount-1 {
		return noneResult()
	}
	L := bp.Line
	lineStart := ctx.Buffer.LineStart(L)
	lineEnd := ctx.Buffer.LineEnd(L)
	nextStart := ctx.Buffer.LineStart(L + 1)
	nextEnd := ctx.Buffer.LineEnd(L + 1)
	textL := ctx.Buffer.Slice(lineStart, lineEnd)
	textNext := ctx.Buffer.Slice(nextStart, nextEnd)
	e := buffer.Edit{Start: lineStart, End: nextEnd, Insert: textNext + "\n" + textL}
	col := c.Position - lineStart
	if col < 0 {
		col = 0
	} else if col > len(textL) {
		col = len(textL)
	}
	newPos := lineStart + len(textNext) + 1 + col
	newC := cursor.Cursor{Position: newPos, Anchor: newPos, ID: c.ID, DesiredCol: c.DesiredCol}
	return command.Result{
		Operation: command.Operation{
			Kind:    command.OperationEditBuffer,
			Edits:   []buffer.Edit{e},
			Cursors: cursor.NewCursorSetFrom([]cursor.Cursor{newC}),
		},
	}
}

var multiLineSpecs = []cmdSpec{
	{name: "edit.clone-line-up", when: "editorFocused && !readOnly", exec: execCloneLineUp},
	{name: "edit.clone-line-down", when: "editorFocused && !readOnly", exec: execCloneLineDown},
	{name: "edit.move-line-up", when: "editorFocused && !readOnly", exec: execMoveLineUp},
	{name: "edit.move-line-down", when: "editorFocused && !readOnly", exec: execMoveLineDown},
}
