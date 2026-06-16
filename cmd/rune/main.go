package main

import (
	"flag"
	"fmt"
	"os"
	"path/filepath"

	ui "rune/pkg/ui"

	tea "charm.land/bubbletea/v2"
)

func main() {
	var workDir string
	flag.StringVar(&workDir, "w", "", "working directory (file tree root)")
	flag.Parse()

	// Resolve file args to absolute paths relative to the shell's CWD.
	rawFiles := flag.Args()
	if len(rawFiles) > 10 {
		rawFiles = rawFiles[:10]
	}
	initialFiles := make([]string, 0, len(rawFiles))
	for _, f := range rawFiles {
		abs, err := filepath.Abs(f)
		if err != nil {
			fmt.Fprintf(os.Stderr, "bad path %q: %v\n", f, err)
			os.Exit(1)
		}
		initialFiles = append(initialFiles, abs)
	}

	var absWorkDir string
	if workDir != "" {
		abs, err := filepath.Abs(workDir)
		if err != nil {
			fmt.Fprintf(os.Stderr, "bad working dir %q: %v\n", workDir, err)
			os.Exit(1)
		}
		if _, err := os.Stat(abs); err != nil {
			fmt.Fprintf(os.Stderr, "working dir %q: %v\n", abs, err)
			os.Exit(1)
		}
		absWorkDir = abs
	}

	app, err := ui.NewApp(absWorkDir, initialFiles)
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
