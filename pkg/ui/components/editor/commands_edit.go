package editor

import (
	"regexp"
	"sort"

	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
)

var leadingWhitespaceRe = regexp.MustCompile(`^[ \t]*`)

type editInfoItem struct {
	edit buffer.Edit
	cID  int
}

func sortInfosDescending(infos []editInfoItem) {
	sort.Slice(infos, func(i, j int) bool {
		if infos[i].edit.Start == infos[j].edit.Start {
			return infos[i].edit.End > infos[j].edit.End
		}
		return infos[i].edit.Start > infos[j].edit.Start
	})
}

func infosToEdits(infos []editInfoItem) []buffer.Edit {
	edits := make([]buffer.Edit, len(infos))
	for i, info := range infos {
		edits[i] = info.edit
	}
	return edits
}

func computePostEditCursors(infos []editInfoItem, insertLen int) []cursor.Cursor {
	var newCursors []cursor.Cursor
	shift := 0
	for i := len(infos) - 1; i >= 0; i-- {
		info := infos[i]
		newPos := info.edit.Start + shift + insertLen
		newCursors = append(newCursors, cursor.Cursor{
			Position: newPos,
			Anchor:   newPos,
			ID:       info.cID,
		})
		shift += insertLen - (info.edit.End - info.edit.Start)
	}
	return newCursors
}

func computePostEditCursorsVar(infos []editInfoItem) []cursor.Cursor {
	var newCursors []cursor.Cursor
	shift := 0
	for i := len(infos) - 1; i >= 0; i-- {
		info := infos[i]
		insLen := len(info.edit.Insert)
		newPos := info.edit.Start + shift + insLen
		newCursors = append(newCursors, cursor.Cursor{
			Position: newPos,
			Anchor:   newPos,
			ID:       info.cID,
		})
		shift += insLen - (info.edit.End - info.edit.Start)
	}
	return newCursors
}

func buildEditResultFromInfos(infos []editInfoItem, insertLen int) command.Result {
	edits := infosToEdits(infos)
	sorted := buffer.CloneAndSortEditsDescending(edits)
	newCursors := computePostEditCursors(infos, insertLen)
	return command.Result{
		Operation: command.Operation{
			Kind:    command.OperationEditBuffer,
			Edits:   sorted,
			Cursors: cursor.NewCursorSetFrom(newCursors),
		},
	}
}

func buildEditResultFromInfosVar(infos []editInfoItem) command.Result {
	edits := infosToEdits(infos)
	sorted := buffer.CloneAndSortEditsDescending(edits)
	newCursors := computePostEditCursorsVar(infos)
	return command.Result{
		Operation: command.Operation{
			Kind:    command.OperationEditBuffer,
			Edits:   sorted,
			Cursors: cursor.NewCursorSetFrom(newCursors),
		},
	}
}

func execInsertChar(ctx command.CommandContext) command.Result {
	char, _ := ctx.Args["char"].(string)
	if char == "" {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	var infos []editInfoItem
	for _, c := range all {
		var e buffer.Edit
		if c.HasSelection() {
			e = buffer.Edit{Start: c.SelectionStart(), End: c.SelectionEnd(), Insert: char}
		} else {
			e = buffer.Edit{Start: c.Position, End: c.Position, Insert: char}
		}
		infos = append(infos, editInfoItem{edit: e, cID: c.ID})
	}

	sortInfosDescending(infos)
	return buildEditResultFromInfos(infos, len(char))
}

func execNewline(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	var infos []editInfoItem
	for _, c := range all {
		pos := c.Position
		if c.HasSelection() {
			pos = c.SelectionStart()
		}
		bp := ctx.Buffer.OffsetToLineCol(pos)
		line := ctx.Buffer.Line(bp.Line)
		indent := leadingWhitespaceRe.FindString(line)
		insertText := "\n" + indent

		var e buffer.Edit
		if c.HasSelection() {
			e = buffer.Edit{Start: c.SelectionStart(), End: c.SelectionEnd(), Insert: insertText}
		} else {
			e = buffer.Edit{Start: c.Position, End: c.Position, Insert: insertText}
		}
		infos = append(infos, editInfoItem{edit: e, cID: c.ID})
	}

	sortInfosDescending(infos)
	return buildEditResultFromInfosVar(infos)
}

func execDeleteLeft(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	var infos []editInfoItem
	for _, c := range all {
		var e buffer.Edit
		if c.HasSelection() {
			e = buffer.Edit{Start: c.SelectionStart(), End: c.SelectionEnd(), Insert: ""}
		} else if c.Position > 0 {
			prev := prevRuneOffset(ctx.Buffer, c.Position)
			e = buffer.Edit{Start: prev, End: c.Position, Insert: ""}
		} else {
			continue
		}
		infos = append(infos, editInfoItem{edit: e, cID: c.ID})
	}

	if len(infos) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	sortInfosDescending(infos)
	return buildEditResultFromInfos(infos, 0)
}

func execDeleteRight(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	var infos []editInfoItem
	for _, c := range all {
		var e buffer.Edit
		if c.HasSelection() {
			e = buffer.Edit{Start: c.SelectionStart(), End: c.SelectionEnd(), Insert: ""}
		} else if c.Position < ctx.Buffer.Len() {
			next := nextRuneOffset(ctx.Buffer, c.Position)
			e = buffer.Edit{Start: c.Position, End: next, Insert: ""}
		} else {
			continue
		}
		infos = append(infos, editInfoItem{edit: e, cID: c.ID})
	}

	if len(infos) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	sortInfosDescending(infos)
	return buildEditResultFromInfos(infos, 0)
}

func execDeleteWordLeft(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	var infos []editInfoItem
	for _, c := range all {
		var e buffer.Edit
		if c.HasSelection() {
			e = buffer.Edit{Start: c.SelectionStart(), End: c.SelectionEnd(), Insert: ""}
		} else if c.Position > 0 {
			wl := wordLeftOffset(ctx.Buffer, c.Position)
			e = buffer.Edit{Start: wl, End: c.Position, Insert: ""}
		} else {
			continue
		}
		infos = append(infos, editInfoItem{edit: e, cID: c.ID})
	}

	if len(infos) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	sortInfosDescending(infos)
	return buildEditResultFromInfos(infos, 0)
}

func execDeleteWordRight(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	var infos []editInfoItem
	for _, c := range all {
		var e buffer.Edit
		if c.HasSelection() {
			e = buffer.Edit{Start: c.SelectionStart(), End: c.SelectionEnd(), Insert: ""}
		} else if c.Position < ctx.Buffer.Len() {
			wr := wordRightOffset(ctx.Buffer, c.Position)
			e = buffer.Edit{Start: c.Position, End: wr, Insert: ""}
		} else {
			continue
		}
		infos = append(infos, editInfoItem{edit: e, cID: c.ID})
	}

	if len(infos) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	sortInfosDescending(infos)
	return buildEditResultFromInfos(infos, 0)
}

func execDeleteLine(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}
	if len(all) > 1 {
		return execDeleteLineMulti(ctx)
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

	sortInfosDescending(infos)
	return buildEditResultFromInfos(infos, 0)
}

func registerEditCommands(builder command.Builder) (command.Builder, error) {
	var err error

	builder, err = builder.Register(command.Command{
		Name:    "edit.insert-character",
		When:    "editorFocused && !readOnly",
		Execute: execInsertChar,
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name:    "edit.newline",
		When:    "editorFocused && !readOnly",
		Execute: execNewline,
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name:    "edit.delete-left",
		When:    "editorFocused && !readOnly",
		Execute: execDeleteLeft,
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name:    "edit.delete-right",
		When:    "editorFocused && !readOnly",
		Execute: execDeleteRight,
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name:    "edit.delete-word-left",
		When:    "editorFocused && !readOnly",
		Execute: execDeleteWordLeft,
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name:    "edit.delete-word-right",
		When:    "editorFocused && !readOnly",
		Execute: execDeleteWordRight,
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name:    "edit.delete-line",
		When:    "editorFocused && !readOnly",
		Execute: execDeleteLine,
	})
	if err != nil {
		return builder, err
	}

	return builder, nil
}
