//go:build fuzzing

package driver

import (
	"fmt"

	"rune/pkg/command"
	"rune/pkg/editor/keybind"
	"rune/pkg/ui/components/textedit"
	"rune/pkg/ui/keymap"
)

// BuildFuzzApp constructs the production command registry + resolver pair
// (mirrors ui.NewApp's steps 1, 3, 4-5, 6-7: RegisterCommands → Build,
// CommandBindings → NewResolver) so every fuzz target exercises the same
// command dispatch production wires, instead of an empty registry paired
// with a nil resolver (under which every editor-internal key — cursor
// motion, Backspace/Delete, Enter-newline, selection, SelectAll, MoveLine,
// AddCursor, per-char edit.insert-character — is a no-op).
//
// ui.NewApp's step 2 (ValidateNoPhysicalKeyCollisions) is skipped: it
// guards against keymap authoring mistakes, not fuzz-target wiring, and
// keymap.Default() is fixed input across every fuzz target.
//
// All three setup errors are surfaced to the caller (f.Fatal / bootstrap
// failure) rather than discarded, per §1.3.
func BuildFuzzApp(keys keymap.Bindings) (command.Registry, keybind.Resolver, error) {
	builder := command.NewBuilder()
	builder, err := textedit.RegisterCommands(builder)
	if err != nil {
		return command.Registry{}, keybind.Resolver{}, fmt.Errorf("registering textedit commands: %w", err)
	}
	registry := builder.Build()

	cmdBindings, err := keys.CommandBindings()
	if err != nil {
		return command.Registry{}, keybind.Resolver{}, fmt.Errorf("building command bindings: %w", err)
	}

	resolver, err := keybind.NewResolver(cmdBindings)
	if err != nil {
		return command.Registry{}, keybind.Resolver{}, fmt.Errorf("building keybind resolver: %w", err)
	}

	return registry, resolver, nil
}
