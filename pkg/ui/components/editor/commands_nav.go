package editor

import "rune/pkg/command"

func registerNavCommands(builder command.Builder) (command.Builder, error) {
        var err error
        builder, err = builder.Register(command.Command{Name: "nav.up", When: "editorFocused", Execute: func(ctx command.CommandContext) command.Result { return command.Result{Operation: command.Operation{Kind: command.OperationNone}} }})
        if err != nil { return builder, err }
        builder, err = builder.Register(command.Command{Name: "nav.down", When: "editorFocused", Execute: func(ctx command.CommandContext) command.Result { return command.Result{Operation: command.Operation{Kind: command.OperationNone}} }})
        if err != nil { return builder, err }
        builder, err = builder.Register(command.Command{Name: "nav.left", When: "editorFocused", Execute: func(ctx command.CommandContext) command.Result { return command.Result{Operation: command.Operation{Kind: command.OperationNone}} }})
        if err != nil { return builder, err }
        builder, err = builder.Register(command.Command{Name: "nav.right", When: "editorFocused", Execute: func(ctx command.CommandContext) command.Result { return command.Result{Operation: command.Operation{Kind: command.OperationNone}} }})
        if err != nil { return builder, err }
        builder, err = builder.Register(command.Command{Name: "nav.line-start", When: "editorFocused", Execute: func(ctx command.CommandContext) command.Result { return command.Result{Operation: command.Operation{Kind: command.OperationNone}} }})
        if err != nil { return builder, err }
        builder, err = builder.Register(command.Command{Name: "nav.line-end", When: "editorFocused", Execute: func(ctx command.CommandContext) command.Result { return command.Result{Operation: command.Operation{Kind: command.OperationNone}} }})
        return builder, err
}
