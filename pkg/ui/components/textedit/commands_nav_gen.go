package textedit

import (
	"rune/pkg/command"
	"rune/pkg/editor/cursor"
)

func execCursorLeft(ctx command.CommandContext) command.Result {
	return handleCursorCmd(ctx, false, func(c cursor.Cursor, _ bool) cursor.Cursor {
		return handleLeftCmd(ctx, c, false, prevRuneOffset)
	})
}

func execCursorRight(ctx command.CommandContext) command.Result {
	return handleCursorCmd(ctx, false, func(c cursor.Cursor, _ bool) cursor.Cursor {
		return handleRightCmd(ctx, c, false, nextRuneOffset)
	})
}

func execCursorUp(ctx command.CommandContext) command.Result {
	return handleRowMoveCmd(ctx, false, -1)
}

func execCursorDown(ctx command.CommandContext) command.Result {
	return handleRowMoveCmd(ctx, false, 1)
}

func execCursorLeftWord(ctx command.CommandContext) command.Result {
	return handleCursorCmd(ctx, false, func(c cursor.Cursor, _ bool) cursor.Cursor {
		return handleLeftCmd(ctx, c, false, wordLeftOffset)
	})
}

func execCursorRightWord(ctx command.CommandContext) command.Result {
	return handleCursorCmd(ctx, false, func(c cursor.Cursor, _ bool) cursor.Cursor {
		return handleRightCmd(ctx, c, false, wordRightOffset)
	})
}

func execCursorBeginLine(ctx command.CommandContext) command.Result {
	return handleCursorCmd(ctx, false, func(c cursor.Cursor, _ bool) cursor.Cursor {
		return handleMoveTo(ctx, c, false, lineStartOffset)
	})
}

func execCursorEndLine(ctx command.CommandContext) command.Result {
	return handleCursorCmd(ctx, false, func(c cursor.Cursor, _ bool) cursor.Cursor {
		return handleMoveTo(ctx, c, false, lineEndOffset)
	})
}

func execSelectLeft(ctx command.CommandContext) command.Result {
	return handleCursorCmd(ctx, true, func(c cursor.Cursor, _ bool) cursor.Cursor {
		return handleLeftCmd(ctx, c, true, prevRuneOffset)
	})
}

func execSelectRight(ctx command.CommandContext) command.Result {
	return handleCursorCmd(ctx, true, func(c cursor.Cursor, _ bool) cursor.Cursor {
		return handleRightCmd(ctx, c, true, nextRuneOffset)
	})
}

func execSelectUp(ctx command.CommandContext) command.Result {
	return handleRowMoveCmd(ctx, true, -1)
}

func execSelectDown(ctx command.CommandContext) command.Result {
	return handleRowMoveCmd(ctx, true, 1)
}

func execSelectLeftWord(ctx command.CommandContext) command.Result {
	return handleCursorCmd(ctx, true, func(c cursor.Cursor, _ bool) cursor.Cursor {
		return handleLeftCmd(ctx, c, true, wordLeftOffset)
	})
}

func execSelectRightWord(ctx command.CommandContext) command.Result {
	return handleCursorCmd(ctx, true, func(c cursor.Cursor, _ bool) cursor.Cursor {
		return handleRightCmd(ctx, c, true, wordRightOffset)
	})
}

func execSelectBeginLine(ctx command.CommandContext) command.Result {
	return handleCursorCmd(ctx, true, func(c cursor.Cursor, _ bool) cursor.Cursor {
		return handleMoveTo(ctx, c, true, lineStartOffset)
	})
}

func execSelectEndLine(ctx command.CommandContext) command.Result {
	return handleCursorCmd(ctx, true, func(c cursor.Cursor, _ bool) cursor.Cursor {
		return handleMoveTo(ctx, c, true, lineEndOffset)
	})
}

// pageStep is the number of rows a page command scrolls: a full viewport
// minus one row of overlap for context. Used for both editable and read-only
// modes so paging behaves identically everywhere.
func pageStep(ctx command.CommandContext) int {
	if ctx.ViewportHeight == nil {
		return 1
	}
	if h := ctx.ViewportHeight(); h > 1 {
		return h - 1
	}
	return 1
}

func execScrollPageUp(ctx command.CommandContext) command.Result {
	return handleRowMoveCmd(ctx, false, -pageStep(ctx))
}

func execScrollPageDown(ctx command.CommandContext) command.Result {
	return handleRowMoveCmd(ctx, false, pageStep(ctx))
}

func execSelectPageUp(ctx command.CommandContext) command.Result {
	return handleRowMoveCmd(ctx, true, -pageStep(ctx))
}

func execSelectPageDown(ctx command.CommandContext) command.Result {
	return handleRowMoveCmd(ctx, true, pageStep(ctx))
}

func execSelectAll(ctx command.CommandContext) command.Result {
	end := ctx.Buffer.Len()
	all := ctx.Cursors.All()
	var c cursor.Cursor
	if len(all) > 0 {
		c = all[0]
	}
	c.Position = end
	c.Anchor = 0
	c.DesiredCol = 0
	return command.Result{
		Operation: command.Operation{
			Kind:    command.OperationMoveCursors,
			Cursors: cursor.NewCursorSetFrom([]cursor.Cursor{c}),
		},
	}
}

var navSpecs = []cmdSpec{
	// Cursor movement (no selection)
	{name: "cursor.character-left", when: "editorFocused", exec: execCursorLeft},
	{name: "cursor.character-right", when: "editorFocused", exec: execCursorRight},
	{name: "cursor.line-up", when: "editorFocused", exec: execCursorUp},
	{name: "cursor.line-down", when: "editorFocused", exec: execCursorDown},
	{name: "cursor.word-left", when: "editorFocused", exec: execCursorLeftWord},
	{name: "cursor.word-right", when: "editorFocused", exec: execCursorRightWord},
	{name: "cursor.line-start", when: "editorFocused", exec: execCursorBeginLine},
	{name: "cursor.line-end", when: "editorFocused", exec: execCursorEndLine},
	{name: "cursor.page-up", when: "editorFocused", exec: execScrollPageUp},
	{name: "cursor.page-down", when: "editorFocused", exec: execScrollPageDown},
	// Selection (extend cursor with anchor)
	{name: "select.character-left", when: "editorFocused", exec: execSelectLeft},
	{name: "select.character-right", when: "editorFocused", exec: execSelectRight},
	{name: "select.line-up", when: "editorFocused", exec: execSelectUp},
	{name: "select.line-down", when: "editorFocused", exec: execSelectDown},
	{name: "select.word-left", when: "editorFocused", exec: execSelectLeftWord},
	{name: "select.word-right", when: "editorFocused", exec: execSelectRightWord},
	{name: "select.line-start", when: "editorFocused", exec: execSelectBeginLine},
	{name: "select.line-end", when: "editorFocused", exec: execSelectEndLine},
	{name: "select.page-up", when: "editorFocused", exec: execSelectPageUp},
	{name: "select.page-down", when: "editorFocused", exec: execSelectPageDown},
	{name: "select.all", when: "editorFocused", exec: execSelectAll},
}
