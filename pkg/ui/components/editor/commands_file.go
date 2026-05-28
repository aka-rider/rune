package editor

import "rune/pkg/command"

func registerFileCommands(builder command.Builder) (command.Builder, error) {
        return builder.Register(command.Command{
                Name: "file.save",
                When: "editorFocused",
                Execute: func(ctx command.CommandContext) command.Result {
                        return command.Result{
                                Operation: command.Operation{
                                        Kind:            command.OperationSaveFile,
                                        Cursors:         ctx.Cursors,
                                        SavePath:        ctx.FilePath,
                                        SaveContent:     ctx.Buffer.Content(),
                                        SaveRequestID:   ctx.NewRequestID(),
                                        SaveContentHash: ctx.HashContent(ctx.Buffer.Content()),
                                },
                        }
                },
        })
}
