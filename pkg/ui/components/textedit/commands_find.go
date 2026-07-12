package textedit

import "rune/pkg/command"

// Find command stubs — these exist solely so app.go startup verification
// finds every keybind's command name. The workspace intercepts find.*
// keybindings (FindOpen/FindNext/FindPrev) before they reach the editor.

func registerFindCommands(builder command.Builder) (command.Builder, error) {
	var err error

	// Register stubs for find.* commands so keymap verification passes.
	stubs := []string{
		"find.open",
		"find.close",
		"find.replace-open",
		"find.replace",
		"find.replace-all",
		"find.next",
		"find.previous",
	}
	for _, name := range stubs {
		builder, err = builder.Register(command.Command{
			Name: name,
			When: "editorFocused",
			Execute: func(ctx command.CommandContext) command.Result {
				return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
			},
		})
		if err != nil {
			return builder, err
		}
	}

	return builder, nil
}
