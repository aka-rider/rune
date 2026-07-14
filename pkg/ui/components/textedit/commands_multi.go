package textedit

import (
	"rune/pkg/command"
	"rune/pkg/editor/cursor"
)

// execMulticursorAdd adds one cursor on the line adjacent (dir=-1 above,
// dir=+1 below) to the extreme (topmost for dir=-1, bottommost for dir=+1)
// existing cursor, preserving its desired column. execMulticursorAddAbove/
// Below are thin direction wrappers below.
func execMulticursorAdd(ctx command.CommandContext, dir int) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return noneResult()
	}

	// Find the extreme cursor to add adjacent to: topmost for dir<0, bottommost for dir>0.
	extreme := all[0]
	for _, c := range all[1:] {
		if (dir < 0 && c.Position < extreme.Position) || (dir > 0 && c.Position > extreme.Position) {
			extreme = c
		}
	}

	bp := ctx.Buffer.OffsetToLineCol(extreme.Position)
	if dir < 0 && bp.Line == 0 {
		return noneResult()
	}
	if dir > 0 && bp.Line >= ctx.Buffer.LineCount()-1 {
		return noneResult()
	}

	targetLine := bp.Line + dir
	desiredCol := extreme.DesiredCol
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

func execMulticursorAddAbove(ctx command.CommandContext) command.Result {
	return execMulticursorAdd(ctx, -1)
}

func execMulticursorAddBelow(ctx command.CommandContext) command.Result {
	return execMulticursorAdd(ctx, 1)
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

var multiSpecs = []cmdSpec{
	{name: "multicursor.add-above", when: "editorFocused", exec: execMulticursorAddAbove},
	{name: "multicursor.add-below", when: "editorFocused", exec: execMulticursorAddBelow},
	{name: "multicursor.escape", when: "editorFocused", exec: execMulticursorEscape},
}
