package editor

import "rune/pkg/command"

func registerEditCommands(builder command.Builder) (command.Builder, error) {
        var err error
        builder, err = builder.Register(command.Command{Name: "edit.delete-left", When: "editorFocused && !readOnly", Execute: func(ctx command.CommandContext) command.Result { return command.Result{Operation: command.Operation{Kind: command.OperationNone}} }})
        if err != nil { return builder, err }
        builder, err = builder.Register(command.Command{Name: "edit.indent", When: "editorFocused && !readOnly", Execute: func(ctx command.CommandContext) command.Result { return command.Result{Operation: command.Operation{Kind: command.OperationNone}} }})
        if err != nil { return builder, err }
        builder, err = builder.Register(command.Command{Name: "edit.outdent", When: "editorFocused && !readOnly", Execute: func(ctx command.CommandContext) command.Result { return command.Result{Operation: command.Operation{Kind: command.OperationNone}} }})
        return builder, err
}
