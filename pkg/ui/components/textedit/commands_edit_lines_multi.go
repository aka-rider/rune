package textedit

import (
	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
)

func execDeleteLineMulti(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	lineCount := ctx.Buffer.LineCount()
	var infos []editInfoItem
	deletedLines := map[int]bool{}

	for _, c := range all {
		bp := ctx.Buffer.OffsetToLineCol(c.Position)
		if deletedLines[bp.Line] {
			continue
		}
		deletedLines[bp.Line] = true

		var e buffer.Edit
		if lineCount == 1 {
			e = buffer.Edit{Start: 0, End: ctx.Buffer.Len(), Insert: ""}
		} else if bp.Line < lineCount-1 {
			e = buffer.Edit{Start: ctx.Buffer.LineStart(bp.Line), End: ctx.Buffer.LineStart(bp.Line + 1), Insert: ""}
		} else {
			e = buffer.Edit{Start: ctx.Buffer.LineEnd(bp.Line - 1), End: ctx.Buffer.LineEnd(bp.Line), Insert: ""}
		}
		infos = append(infos, editInfoItem{edit: e, cID: c.ID})
	}

	if len(infos) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	sortInfosDescending(infos)
	return buildEditResultFromInfos(infos, 0)
}

func execCloneLineUp(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	var infos []editInfoItem
	for _, c := range all {
		bp := ctx.Buffer.OffsetToLineCol(c.Position)
		if bp.Line == 0 {
			continue
		}
		lineStart := ctx.Buffer.LineStart(bp.Line)
		lineEnd := ctx.Buffer.LineEnd(bp.Line)
		lineText := ctx.Buffer.Slice(lineStart, lineEnd)
		e := buffer.Edit{Start: lineStart, End: lineStart, Insert: lineText + "\n"}
		infos = append(infos, editInfoItem{edit: e, cID: c.ID})
	}

	if len(infos) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	sortInfosDescending(infos)
	return buildEditResultFromInfosVar(infos)
}

func execCloneLineDown(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	var infos []editInfoItem
	for _, c := range all {
		bp := ctx.Buffer.OffsetToLineCol(c.Position)
		lineEnd := ctx.Buffer.LineEnd(bp.Line)
		e := buffer.Edit{Start: lineEnd, End: lineEnd, Insert: "\n" + ctx.Buffer.Slice(ctx.Buffer.LineStart(bp.Line), lineEnd)}
		infos = append(infos, editInfoItem{edit: e, cID: c.ID})
	}

	if len(infos) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	sortInfosDescending(infos)
	return buildEditResultFromInfosVar(infos)
}

func execMoveLineUp(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}
	c := all[0]
	bp := ctx.Buffer.OffsetToLineCol(c.Position)
	if bp.Line == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
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
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}
	c := all[0]
	bp := ctx.Buffer.OffsetToLineCol(c.Position)
	lineCount := ctx.Buffer.LineCount()
	if bp.Line >= lineCount-1 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
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

func registerMultiLineCommands(builder command.Builder) (command.Builder, error) {
	var err error

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

	return builder, nil
}
