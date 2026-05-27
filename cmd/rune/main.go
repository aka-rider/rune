package main

import (
	"fmt"
	"os"

	ui "rune/pkg/ui"

	tea "charm.land/bubbletea/v2"
)

func main() {
	app, err := ui.NewApp()
	if err != nil {
		fmt.Fprintf(os.Stderr, "failed to start: %v\n", err)
		os.Exit(1)
	}

	p := tea.NewProgram(app)
	if _, err := p.Run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
