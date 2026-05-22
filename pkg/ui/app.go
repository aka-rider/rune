package ui

import (
	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/keymap"
	"rune/pkg/ui/pages/workspace"
	"rune/pkg/ui/styles"
)

// Model is the only type that satisfies tea.Model (returns tea.Model from Update).
// All internal types use concrete return types to avoid interface boxing.
type Model struct{ ws workspace.Model }

func DefaultApp() Model {
	return Model{ws: workspace.New(keymap.Default(), styles.Default())}
}

func (m Model) Init() tea.Cmd { return m.ws.Init() }

func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	var cmd tea.Cmd
	m.ws, cmd = m.ws.Update(msg)
	return m, cmd
}

func (m Model) View() tea.View { return m.ws.View() }
