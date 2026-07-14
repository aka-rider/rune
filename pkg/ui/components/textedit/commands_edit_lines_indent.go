package textedit

import (
	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
)

func execIndentLine(ctx command.CommandContext) command.Result {
	return perLineEdits(ctx, true, func(line int, c cursor.Cursor) (buffer.Edit, bool) {
		lineStart := ctx.Buffer.LineStart(line)
		return buffer.Edit{Start: lineStart, End: lineStart, Insert: "\t"}, true
	})
}

func execDedentLine(ctx command.CommandContext) command.Result {
	return perLineEdits(ctx, true, func(line int, c cursor.Cursor) (buffer.Edit, bool) {
		lineStart := ctx.Buffer.LineStart(line)
		lineEnd := ctx.Buffer.LineEnd(line)
		lineText := ctx.Buffer.Slice(lineStart, lineEnd)

		// Find leading whitespace
		indentEnd := 0
		for i, r := range lineText {
			if r == '\t' || r == ' ' {
				indentEnd = i + 1
			} else {
				break
			}
		}

		if indentEnd == 0 {
			return buffer.Edit{}, false
		}

		// Remove up to one tab or 4 spaces
		remove := 1
		if indentEnd >= 4 {
			// Check if it's 4 spaces
			spaceRun := 0
			for i := 0; i < len(lineText) && lineText[i] == ' '; i++ {
				spaceRun++
			}
			if spaceRun >= 4 {
				remove = 4
			}
		}
		if indentEnd < remove {
			remove = indentEnd
		}
		return buffer.Edit{Start: lineStart, End: lineStart + remove, Insert: ""}, true
	})
}

var indentSpecs = []cmdSpec{
	{name: "edit.indent", when: "editorFocused && !readOnly", exec: execIndentLine},
	{name: "edit.outdent", when: "editorFocused && !readOnly", exec: execDedentLine},
}
