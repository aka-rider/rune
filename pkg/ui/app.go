package ui

import (
	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/keymap"
	"rune/pkg/ui/pages/workspace"
	"rune/pkg/ui/styles"
)

// Model is the top-level tea.Model, delegating to the workspace page.
type Model struct{ ws workspace.Model }

func DefaultApp() Model {
	return Model{ws: workspace.New(keymap.Default(), styles.Default())}
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
	return v
}
