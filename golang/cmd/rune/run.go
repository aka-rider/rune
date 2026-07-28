//go:build !fuzzing

package main

import (
	tea "charm.land/bubbletea/v2"

	ui "rune/pkg/ui"
)

func run(app ui.Model) error {
	p := tea.NewProgram(app)
	_, err := p.Run()
	return err
}
