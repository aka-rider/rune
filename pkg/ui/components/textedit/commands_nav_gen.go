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
	return handleCursorCmd(ctx, false, func(c cursor.Cursor, selectMode bool) cursor.Cursor {
		return moveRow(ctx, c, -1, selectMode)
	})
}

func execCursorDown(ctx command.CommandContext) command.Result {
	return handleCursorCmd(ctx, false, func(c cursor.Cursor, selectMode bool) cursor.Cursor {
		return moveRow(ctx, c, 1, selectMode)
	})
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

func execCursorBeginDocument(ctx command.CommandContext) command.Result {
	return handleCursorCmd(ctx, false, func(c cursor.Cursor, _ bool) cursor.Cursor {
		return updateHorizontal(ctx, c, 0, false)
	})
}

func execCursorEndDocument(ctx command.CommandContext) command.Result {
	return handleCursorCmd(ctx, false, func(c cursor.Cursor, _ bool) cursor.Cursor {
		return updateHorizontal(ctx, c, ctx.Buffer.Len(), false)
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
	return handleCursorCmd(ctx, true, func(c cursor.Cursor, selectMode bool) cursor.Cursor {
		return moveRow(ctx, c, -1, selectMode)
	})
}

func execSelectDown(ctx command.CommandContext) command.Result {
	return handleCursorCmd(ctx, true, func(c cursor.Cursor, selectMode bool) cursor.Cursor {
		return moveRow(ctx, c, 1, selectMode)
	})
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

func execSelectBeginDocument(ctx command.CommandContext) command.Result {
	return handleCursorCmd(ctx, true, func(c cursor.Cursor, _ bool) cursor.Cursor {
		return updateHorizontal(ctx, c, 0, true)
	})
}

func execSelectEndDocument(ctx command.CommandContext) command.Result {
	return handleCursorCmd(ctx, true, func(c cursor.Cursor, _ bool) cursor.Cursor {
		return updateHorizontal(ctx, c, ctx.Buffer.Len(), true)
	})
}

func execScrollPageUp(ctx command.CommandContext) command.Result {
	return handleScrollCmd(ctx, 0, -1)
}

func execScrollPageDown(ctx command.CommandContext) command.Result {
	return handleScrollCmd(ctx, 0, 1)
}

func execScrollToTop(ctx command.CommandContext) command.Result {
	return handleScrollCmd(ctx, 0, -1000)
}

func execScrollToBottom(ctx command.CommandContext) command.Result {
	return handleScrollCmd(ctx, 0, 1000)
}

func execScrollLeft(ctx command.CommandContext) command.Result {
	return handleScrollCmd(ctx, -1, 0)
}

func execScrollRight(ctx command.CommandContext) command.Result {
	return handleScrollCmd(ctx, 1, 0)
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

func registerNavCommands(builder command.Builder) (command.Builder, error) {
	var err error

	type entry struct {
		name string
		when string
		exec func(command.CommandContext) command.Result
	}

	cmds := []entry{
		// Cursor movement (no selection)
		{"cursor.character-left", "editorFocused", execCursorLeft},
		{"cursor.character-right", "editorFocused", execCursorRight},
		{"cursor.line-up", "editorFocused", execCursorUp},
		{"cursor.line-down", "editorFocused", execCursorDown},
		{"cursor.word-left", "editorFocused", execCursorLeftWord},
		{"cursor.word-right", "editorFocused", execCursorRightWord},
		{"cursor.line-start", "editorFocused", execCursorBeginLine},
		{"cursor.line-end", "editorFocused", execCursorEndLine},
		{"cursor.page-up", "editorFocused", execScrollPageUp},
		{"cursor.page-down", "editorFocused", execScrollPageDown},
		// Selection (extend cursor with anchor)
		{"select.character-left", "editorFocused", execSelectLeft},
		{"select.character-right", "editorFocused", execSelectRight},
		{"select.line-up", "editorFocused", execSelectUp},
		{"select.line-down", "editorFocused", execSelectDown},
		{"select.word-left", "editorFocused", execSelectLeftWord},
		{"select.word-right", "editorFocused", execSelectRightWord},
		{"select.line-start", "editorFocused", execSelectBeginLine},
		{"select.line-end", "editorFocused", execSelectEndLine},
		{"select.page-up", "editorFocused", execScrollPageUp},
		{"select.page-down", "editorFocused", execScrollPageDown},
		{"select.all", "editorFocused", execSelectAll},
	}

	for _, c := range cmds {
		builder, err = builder.Register(command.Command{
			Name:    c.name,
			When:    c.when,
			Execute: c.exec,
		})
		if err != nil {
			return builder, err
		}
	}

	return builder, nil
}
