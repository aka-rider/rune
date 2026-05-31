package ui

import (
	"fmt"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/command"
	"rune/pkg/editor/keybind"
	"rune/pkg/terminal"
	"rune/pkg/ui/components/editor"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/pages/workspace"
	"rune/pkg/ui/styles"
)

// Model is the top-level tea.Model, delegating to the workspace page.
type Model struct{ ws workspace.Model }

// NewApp initializes the application state, commands, and keybindings.
func NewApp() (Model, error) {
	keys := keymap.Default()
	st := styles.Default()

	// 1. Build immutable command registry
	builder := command.NewBuilder()
	builder, err := editor.RegisterCommands(builder)
	if err != nil {
		return Model{}, fmt.Errorf("registering editor commands: %w", err)
	}
	registry := builder.Build()

	// 2. Validate physical key collisions
	if err := keys.ValidateNoPhysicalKeyCollisions(); err != nil {
		return Model{}, fmt.Errorf("keymap validation: %w", err)
	}

	// 3. Build keymap command bindings
	cmdBindings, err := keys.CommandBindings()
	if err != nil {
		return Model{}, fmt.Errorf("building command bindings: %w", err)
	}

	// 4 & 5. Verify bindings against registry
	for i, b := range cmdBindings {
		cmd, ok := registry.Get(b.Command)
		if !ok {
			return Model{}, fmt.Errorf("binding references unknown command %q", b.Command)
		}
		if b.When != "" && b.When != cmd.When {
			return Model{}, fmt.Errorf("binding %q predicate %q does not match command predicate %q", b.Command, b.When, cmd.When)
		}
		if b.When == "" {
			cmdBindings[i].When = cmd.When
		}
	}

	// 6 & 7. Call keybind.NewResolver
	resolver, err := keybind.NewResolver(cmdBindings)
	if err != nil {
		return Model{}, fmt.Errorf("building keybind resolver: %w", err)
	}

	// 8. Detect terminal capabilities
	caps := terminal.DetectCapabilities()

	ws := workspace.New(keys, st, registry, resolver, caps)
	return Model{ws: ws}, nil
}

func (m Model) Init() tea.Cmd { return m.ws.Init() }

func (m Model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	var cmd tea.Cmd
	m.ws, cmd = m.ws.Update(msg)
	return m, cmd
}

func (m Model) View() tea.View {
	v := m.ws.View()
	v.AltScreen = true
	v.MouseMode = tea.MouseModeAllMotion
	return v
}
