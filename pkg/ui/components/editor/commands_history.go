package editor

import "rune/pkg/command"

func registerHistoryCommands(builder command.Builder) (command.Builder, error) {
	var err error
	builder, err = builder.Register(command.Command{
		Name:     "history.undo",
		Category: "history",
		Title:    "Undo",
		When:     "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return command.Result{
				Operation: command.Operation{
					Kind: command.OperationHistory,
				},
			}
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name:     "history.redo",
		Category: "history",
		Title:    "Redo",
		When:     "editorFocused",
		Execute: func(ctx command.CommandContext) command.Result {
			return command.Result{
				Operation: command.Operation{
					Kind: command.OperationHistory,
				},
			}
		},
	})
	if err != nil {
		return builder, err
	}

	return builder, nil
}
