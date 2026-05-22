package main

import (
	"fmt"
	"os"

	tea "charm.land/bubbletea/v2"
	ui "rune/pkg/ui"
)

func main() {
	p := tea.NewProgram(ui.DefaultApp(), tea.WithAltScreen())
	if _, err := p.Run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
