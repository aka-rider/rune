package textedit

import (
	"regexp"
	"sort"

	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
)

var leadingWhitespaceRe = regexp.MustCompile(`^[ \t]*`)

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
