package editor

import "rune/pkg/command"

func registerFindCommands(builder command.Builder) (command.Builder, error) {
	var err error

	builder, err = builder.Register(command.Command{
		Name:     "find.open",
		Category: "find",
		Title:    "Find",
		When:     "editorFocused",
		Execute:  execFindOpen,
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name:     "find.replace-open",
		Category: "find",
		Title:    "Find and Replace",
		When:     "editorFocused",
		Execute:  execFindReplaceOpen,
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name:     "find.next",
		Category: "find",
		Title:    "Find Next",
		When:     "editorFocused",
		Execute:  execFindNext,
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name:     "find.previous",
		Category: "find",
		Title:    "Find Previous",
		When:     "editorFocused",
		Execute:  execFindPrevious,
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name:     "find.replace",
		Category: "find",
		Title:    "Replace",
		When:     "editorFocused",
		Execute:  execFindReplace,
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name:     "find.replace-all",
		Category: "find",
		Title:    "Replace All",
		When:     "editorFocused",
		Execute:  execFindReplaceAll,
	})
	if err != nil {
		return builder, err
	}

	return builder, nil
}

func execFindOpen(_ command.CommandContext) command.Result {
	return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
}

func execFindReplaceOpen(_ command.CommandContext) command.Result {
	return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
}

func execFindNext(_ command.CommandContext) command.Result {
	return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
}

func execFindPrevious(_ command.CommandContext) command.Result {
	return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
}

func execFindReplace(_ command.CommandContext) command.Result {
	return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
}

func execFindReplaceAll(_ command.CommandContext) command.Result {
	return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
}
