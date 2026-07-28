package textedit

import (
	"rune/pkg/command"
	"rune/pkg/editor/cursor"
)

func execInsertChar(ctx command.CommandContext) command.Result {
	char, _ := ctx.Args["char"].(string)
	if char == "" {
		return noneResult()
	}
	return perCursorSelectionEdits(ctx,
		func(i int, c cursor.Cursor) string { return char },
		func(c cursor.Cursor) (int, int, bool) { return c.Position, c.Position, true })
}

func execNewline(ctx command.CommandContext) command.Result {
	return perCursorSelectionEdits(ctx,
		func(i int, c cursor.Cursor) string {
			pos := c.Position
			if c.HasSelection() {
				pos = c.SelectionStart()
			}
			bp := ctx.Buffer.OffsetToLineCol(pos)
			line := ctx.Buffer.Line(bp.Line)
			indent := leadingWhitespaceRe.FindString(line)
			return "\n" + indent
		},
		func(c cursor.Cursor) (int, int, bool) { return c.Position, c.Position, true })
}

func execDeleteLeft(ctx command.CommandContext) command.Result {
	return perCursorSelectionEdits(ctx,
		func(i int, c cursor.Cursor) string { return "" },
		func(c cursor.Cursor) (int, int, bool) {
			if c.Position <= 0 {
				return 0, 0, false
			}
			return prevRuneOffset(ctx.Buffer, c.Position), c.Position, true
		})
}

func execDeleteRight(ctx command.CommandContext) command.Result {
	return perCursorSelectionEdits(ctx,
		func(i int, c cursor.Cursor) string { return "" },
		func(c cursor.Cursor) (int, int, bool) {
			if c.Position >= ctx.Buffer.Len() {
				return 0, 0, false
			}
			return c.Position, nextRuneOffset(ctx.Buffer, c.Position), true
		})
}

func execDeleteWordLeft(ctx command.CommandContext) command.Result {
	return perCursorSelectionEdits(ctx,
		func(i int, c cursor.Cursor) string { return "" },
		func(c cursor.Cursor) (int, int, bool) {
			if c.Position <= 0 {
				return 0, 0, false
			}
			return wordLeftOffset(ctx.Buffer, c.Position), c.Position, true
		})
}

func execDeleteWordRight(ctx command.CommandContext) command.Result {
	return perCursorSelectionEdits(ctx,
		func(i int, c cursor.Cursor) string { return "" },
		func(c cursor.Cursor) (int, int, bool) {
			if c.Position >= ctx.Buffer.Len() {
				return 0, 0, false
			}
			return c.Position, wordRightOffset(ctx.Buffer, c.Position), true
		})
}

var editSpecs = []cmdSpec{
	{name: "edit.insert-character", when: "editorFocused && !readOnly", exec: execInsertChar},
	{name: "edit.newline", when: "editorFocused && !readOnly", exec: execNewline},
	{name: "edit.delete-left", when: "editorFocused && !readOnly", exec: execDeleteLeft},
	{name: "edit.delete-right", when: "editorFocused && !readOnly", exec: execDeleteRight},
	{name: "edit.delete-word-left", when: "editorFocused && !readOnly", exec: execDeleteWordLeft},
	{name: "edit.delete-word-right", when: "editorFocused && !readOnly", exec: execDeleteWordRight},
	{name: "edit.delete-line", when: "editorFocused && !readOnly", exec: execDeleteLineMulti},
}
