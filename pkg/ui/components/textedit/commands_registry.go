package textedit

import "rune/pkg/command"

func RegisterCommands(builder command.Builder) (command.Builder, error) {
	var err error
	builder, err = registerNavCommands(builder)
	if err != nil {
		return builder, err
	}
	builder, err = registerEditCommands(builder)
	if err != nil {
		return builder, err
	}
	builder, err = registerIndentCommands(builder)
	if err != nil {
		return builder, err
	}
	builder, err = registerMultiLineCommands(builder)
	if err != nil {
		return builder, err
	}
	builder, err = registerMultiCommands(builder)
	if err != nil {
		return builder, err
	}
	builder, err = registerHistoryCommands(builder)
	if err != nil {
		return builder, err
	}
	builder, err = registerClipboardCommands(builder)
	if err != nil {
		return builder, err
	}
	builder, err = registerFindCommands(builder)
	if err != nil {
		return builder, err
	}
	return builder, nil
}
