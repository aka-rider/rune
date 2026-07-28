package markdownedit

import (
	"rune/pkg/command"
	"rune/pkg/ui/components/textedit"
)

// RegisterCommands registers all markdownedit commands (same set as textedit).
func RegisterCommands(builder command.Builder) (command.Builder, error) {
	return textedit.RegisterCommands(builder)
}
