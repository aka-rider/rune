package textedit

import "rune/pkg/command"

func registerHistoryCommands(builder command.Builder) (command.Builder, error) {
	var err error

	builder, err = builder.Register(command.Command{
		Name:    "history.undo",
		When:    "editorFocused && !readOnly",
		Execute: func(ctx command.CommandContext) command.Result {
			return command.Result{Operation: command.Operation{Kind: command.OperationHistory}}
		},
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name:    "history.redo",
		When:    "editorFocused && !readOnly",
		Execute: func(ctx command.CommandContext) command.Result {
			return command.Result{Operation: command.Operation{Kind: command.OperationHistory}}
		},
	})
	if err != nil {
		return builder, err
	}

	return builder, nil
}
