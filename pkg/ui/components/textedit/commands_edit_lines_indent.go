package textedit

import (
	"rune/pkg/command"
	"rune/pkg/editor/buffer"
)

func execIndentLine(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	var infos []editInfoItem
	seen := map[int]bool{}
	for _, c := range all {
		bp := ctx.Buffer.OffsetToLineCol(c.Position)
		if seen[bp.Line] {
			continue
		}
		seen[bp.Line] = true
		lineStart := ctx.Buffer.LineStart(bp.Line)
		e := buffer.Edit{Start: lineStart, End: lineStart, Insert: "\t"}
		infos = append(infos, editInfoItem{edit: e, cID: c.ID})
	}

	if len(infos) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	sortInfosDescending(infos)
	return buildEditResultFromInfos(infos, 1)
}

func execDedentLine(ctx command.CommandContext) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	var infos []editInfoItem
	seen := map[int]bool{}
	for _, c := range all {
		bp := ctx.Buffer.OffsetToLineCol(c.Position)
		if seen[bp.Line] {
			continue
		}
		seen[bp.Line] = true

		lineStart := ctx.Buffer.LineStart(bp.Line)
		lineEnd := ctx.Buffer.LineEnd(bp.Line)
		line := ctx.Buffer.Slice(lineStart, lineEnd)

		// Find leading whitespace
		indentEnd := 0
		for i, r := range line {
			if r == '\t' || r == ' ' {
				indentEnd = i + 1
			} else {
				break
			}
		}

		if indentEnd > 0 {
			// Remove up to one tab or 4 spaces
			remove := 1
			if indentEnd >= 4 {
				// Check if it's 4 spaces
				spaceRun := 0
				for i := 0; i < len(line) && line[i] == ' '; i++ {
					spaceRun++
				}
				if spaceRun >= 4 {
					remove = 4
				}
			}
			if indentEnd < remove {
				remove = indentEnd
			}
			e := buffer.Edit{Start: lineStart, End: lineStart + remove, Insert: ""}
			infos = append(infos, editInfoItem{edit: e, cID: c.ID})
		}
	}

	if len(infos) == 0 {
		return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
	}

	sortInfosDescending(infos)
	return buildEditResultFromInfos(infos, 0)
}

func registerIndentCommands(builder command.Builder) (command.Builder, error) {
	var err error

	builder, err = builder.Register(command.Command{
		Name:    "edit.indent",
		When:    "editorFocused && !readOnly",
		Execute: execIndentLine,
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name:    "edit.outdent",
		When:    "editorFocused && !readOnly",
		Execute: execDedentLine,
	})
	if err != nil {
		return builder, err
	}

	return builder, nil
}
