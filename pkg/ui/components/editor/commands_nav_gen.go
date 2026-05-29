package editor

import (
	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
)

func registerNavCommands(builder command.Builder) (command.Builder, error) {
	var err error

	builder, err = builder.Register(command.Command{
		Name: "cursor.character-left",
		When: "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return handleCursorCmd(ctx, false, func(c cursor.Cursor, selectMode bool) cursor.Cursor {
				return handleLeftCmd(ctx, c, selectMode, prevRuneOffset)
			})
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name: "select.character-left",
		When: "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return handleCursorCmd(ctx, true, func(c cursor.Cursor, selectMode bool) cursor.Cursor {
				return handleLeftCmd(ctx, c, selectMode, prevRuneOffset)
			})
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name: "cursor.character-right",
		When: "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return handleCursorCmd(ctx, false, func(c cursor.Cursor, selectMode bool) cursor.Cursor {
				return handleRightCmd(ctx, c, selectMode, nextRuneOffset)
			})
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name: "select.character-right",
		When: "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return handleCursorCmd(ctx, true, func(c cursor.Cursor, selectMode bool) cursor.Cursor {
				return handleRightCmd(ctx, c, selectMode, nextRuneOffset)
			})
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name: "cursor.line-up",
		When: "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return handleCursorCmd(ctx, false, func(c cursor.Cursor, selectMode bool) cursor.Cursor {
				return moveRow(ctx, c, -1, selectMode)
			})
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name: "select.line-up",
		When: "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return handleCursorCmd(ctx, true, func(c cursor.Cursor, selectMode bool) cursor.Cursor {
				return moveRow(ctx, c, -1, selectMode)
			})
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name: "cursor.line-down",
		When: "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return handleCursorCmd(ctx, false, func(c cursor.Cursor, selectMode bool) cursor.Cursor {
				return moveRow(ctx, c, 1, selectMode)
			})
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name: "select.line-down",
		When: "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return handleCursorCmd(ctx, true, func(c cursor.Cursor, selectMode bool) cursor.Cursor {
				return moveRow(ctx, c, 1, selectMode)
			})
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name: "cursor.word-left",
		When: "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return handleCursorCmd(ctx, false, func(c cursor.Cursor, selectMode bool) cursor.Cursor {
				return handleMoveTo(ctx, c, selectMode, wordLeftOffset)
			})
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name: "select.word-left",
		When: "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return handleCursorCmd(ctx, true, func(c cursor.Cursor, selectMode bool) cursor.Cursor {
				return handleMoveTo(ctx, c, selectMode, wordLeftOffset)
			})
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name: "cursor.word-right",
		When: "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return handleCursorCmd(ctx, false, func(c cursor.Cursor, selectMode bool) cursor.Cursor {
				return handleMoveTo(ctx, c, selectMode, wordRightOffset)
			})
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name: "select.word-right",
		When: "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return handleCursorCmd(ctx, true, func(c cursor.Cursor, selectMode bool) cursor.Cursor {
				return handleMoveTo(ctx, c, selectMode, wordRightOffset)
			})
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name: "cursor.line-start",
		When: "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return handleCursorCmd(ctx, false, func(c cursor.Cursor, selectMode bool) cursor.Cursor {
				return handleMoveTo(ctx, c, selectMode, lineStartOffset)
			})
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name: "select.line-start",
		When: "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return handleCursorCmd(ctx, true, func(c cursor.Cursor, selectMode bool) cursor.Cursor {
				return handleMoveTo(ctx, c, selectMode, lineStartOffset)
			})
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name: "cursor.line-end",
		When: "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return handleCursorCmd(ctx, false, func(c cursor.Cursor, selectMode bool) cursor.Cursor {
				return handleMoveTo(ctx, c, selectMode, lineEndOffset)
			})
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name: "select.line-end",
		When: "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return handleCursorCmd(ctx, true, func(c cursor.Cursor, selectMode bool) cursor.Cursor {
				return handleMoveTo(ctx, c, selectMode, lineEndOffset)
			})
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name: "cursor.document-start",
		When: "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return handleCursorCmd(ctx, false, func(c cursor.Cursor, selectMode bool) cursor.Cursor {
				return handleMoveTo(ctx, c, selectMode, func(b buffer.Buffer, i int) int { return 0 })
			})
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name: "select.document-start",
		When: "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return handleCursorCmd(ctx, true, func(c cursor.Cursor, selectMode bool) cursor.Cursor {
				return handleMoveTo(ctx, c, selectMode, func(b buffer.Buffer, i int) int { return 0 })
			})
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name: "cursor.document-end",
		When: "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return handleCursorCmd(ctx, false, func(c cursor.Cursor, selectMode bool) cursor.Cursor {
				return handleMoveTo(ctx, c, selectMode, func(b buffer.Buffer, i int) int { return ctx.Buffer.Len() })
			})
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name: "select.document-end",
		When: "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return handleCursorCmd(ctx, true, func(c cursor.Cursor, selectMode bool) cursor.Cursor {
				return handleMoveTo(ctx, c, selectMode, func(b buffer.Buffer, i int) int { return ctx.Buffer.Len() })
			})
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name: "select.all",
		When: "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return command.Result{
				Operation: command.Operation{
					Kind:    command.OperationMoveCursors,
					Cursors: cursor.NewCursorSetFrom([]cursor.Cursor{{Position: ctx.Buffer.Len(), Anchor: 0, ID: 1}}),
				},
			}
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name: "scroll.line-up",
		When: "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return handleScrollCmd(ctx, 0, -1)
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name: "scroll.line-down",
		When: "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return handleScrollCmd(ctx, 0, 1)
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name: "scroll.character-left",
		When: "editorFocused && !softWrap",
		Execute: func(ctx command.CommandContext) command.Result {
			if ctx.SoftWrap != nil && ctx.SoftWrap() {
				return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
			}
			return handleScrollCmd(ctx, -1, 0)
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name: "scroll.character-right",
		When: "editorFocused && !softWrap",
		Execute: func(ctx command.CommandContext) command.Result {
			if ctx.SoftWrap != nil && ctx.SoftWrap() {
				return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
			}
			return handleScrollCmd(ctx, 1, 0)
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name: "cursor.page-up",
		When: "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return handleCursorCmd(ctx, false, func(c cursor.Cursor, selectMode bool) cursor.Cursor {
				delta := 0
				if ctx.ViewportHeight != nil {
					delta = -ctx.ViewportHeight() + 2
				}
				return moveRow(ctx, c, delta, selectMode)
			})
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name: "select.page-up",
		When: "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return handleCursorCmd(ctx, true, func(c cursor.Cursor, selectMode bool) cursor.Cursor {
				delta := 0
				if ctx.ViewportHeight != nil {
					delta = -ctx.ViewportHeight() + 2
				}
				return moveRow(ctx, c, delta, selectMode)
			})
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name: "cursor.page-down",
		When: "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return handleCursorCmd(ctx, false, func(c cursor.Cursor, selectMode bool) cursor.Cursor {
				delta := 0
				if ctx.ViewportHeight != nil {
					delta = ctx.ViewportHeight() - 2
				}
				return moveRow(ctx, c, delta, selectMode)
			})
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name: "select.page-down",
		When: "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return handleCursorCmd(ctx, true, func(c cursor.Cursor, selectMode bool) cursor.Cursor {
				delta := 0
				if ctx.ViewportHeight != nil {
					delta = ctx.ViewportHeight() - 2
				}
				return moveRow(ctx, c, delta, selectMode)
			})
		},
	})
	if err != nil {
		return builder, err
	}

	return builder, err
}
