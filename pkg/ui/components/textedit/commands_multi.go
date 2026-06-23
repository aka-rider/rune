package textedit

import (
	"rune/pkg/command"
	"rune/pkg/editor/cursor"
)

func execMulticursorAddAbove(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	// Find the topmost cursor to add above
	topmost := all[0]
	for _, c := range all[1:] {
		if c.Position < topmost.Position {
			topmost = c
		}
	}

	bp := ctx.Buffer.OffsetToLineCol(topmost.Position)
	if bp.Line == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	targetLine := bp.Line - 1
	desiredCol := topmost.DesiredCol
	if desiredCol == 0 {
		desiredCol = bp.Col
	}

	lineLen := ctx.Buffer.LineEnd(targetLine) - ctx.Buffer.LineStart(targetLine)
	col := desiredCol
	if col > lineLen {
		col = lineLen
	}

	newOffset := ctx.Buffer.LineStart(targetLine) + col
	newCursor := cursor.Cursor{
		Position:   newOffset,
		Anchor:     newOffset,
		DesiredCol: desiredCol,
	}

	newSet := ctx.Cursors.Add(newCursor)
	return command.Result{
		Operation: command.Operation{
			Kind:    command.OperationMoveCursors,
			Cursors: newSet,
		},
	}
}

func execMulticursorAddBelow(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	// Find the bottommost cursor to add below
	bottommost := all[0]
	for _, c := range all[1:] {
		if c.Position > bottommost.Position {
			bottommost = c
		}
	}

	bp := ctx.Buffer.OffsetToLineCol(bottommost.Position)
	if bp.Line >= ctx.Buffer.LineCount()-1 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	targetLine := bp.Line + 1
	desiredCol := bottommost.DesiredCol
	if desiredCol == 0 {
		desiredCol = bp.Col
	}

	lineLen := ctx.Buffer.LineEnd(targetLine) - ctx.Buffer.LineStart(targetLine)
	col := desiredCol
	if col > lineLen {
		col = lineLen
	}

	newOffset := ctx.Buffer.LineStart(targetLine) + col
	newCursor := cursor.Cursor{
		Position:   newOffset,
		Anchor:     newOffset,
		DesiredCol: desiredCol,
	}

	newSet := ctx.Cursors.Add(newCursor)
	return command.Result{
		Operation: command.Operation{
			Kind:    command.OperationMoveCursors,
			Cursors: newSet,
		},
	}
}

func execMulticursorEscape(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	// If multi-cursor, collapse to single (keep primary)
	if ctx.Cursors.IsMulti() {
		primary := ctx.Cursors.Primary()
		return command.Result{
			Operation: command.Operation{
				Kind:    command.OperationMoveCursors,
				Cursors: ctx.Cursors.CollapseTo(primary),
			},
		}
	}

	// If single cursor with selection, collapse selection
	primary := ctx.Cursors.Primary()
	if primary.HasSelection() {
		collapsed := primary.CollapseToPosition()
		return command.Result{
			Operation: command.Operation{
				Kind:    command.OperationMoveCursors,
				Cursors: cursor.NewCursorSetFrom([]cursor.Cursor{collapsed}),
			},
		}
	}

	// Neither multi-cursor nor selection — propagate (no-op from command perspective)
	return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
}

func registerMultiCommands(builder command.Builder) (command.Builder, error) {
	var err error

	builder, err = builder.Register(command.Command{
		Name:    "multicursor.add-above",
		When:    "editorFocused",
		Execute: execMulticursorAddAbove,
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name:    "multicursor.add-below",
		When:    "editorFocused",
		Execute: execMulticursorAddBelow,
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name:    "multicursor.escape",
		When:    "editorFocused",
		Execute: execMulticursorEscape,
	})
	if err != nil {
		return builder, err
	}

	return builder, nil
}
