//go:build fuzzing

package main

import (
	"flag"
	"fmt"
	"os"
	"time"

	"rune/internal/fuzz/artifact"
	"rune/internal/fuzz/driver"
	"rune/internal/fuzz/event"
	"rune/pkg/command"
	"rune/pkg/docstate"
	"rune/pkg/editor/keybind"
	"rune/pkg/terminal"
	ui "rune/pkg/ui"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/pages/workspace"
	"rune/pkg/ui/styles"
)

var fuzzScript string

func init() {
	flag.StringVar(&fuzzScript, "fuzz-script", "", "path to keys.jsonl seed file")
}

// run replaces the tea.Program entry point under -tags fuzzing.
// It loads a fuzz script and drives the workspace synchronously.
func run(_ ui.Model) error {
	if fuzzScript == "" {
		fmt.Fprintln(os.Stderr, "rune-fuzz requires --fuzz-script=<path>")
		os.Exit(1)
	}

	events, err := event.LoadJSONL(fuzzScript)
	if err != nil {
		return fmt.Errorf("load script: %w", err)
	}

	store, err := docstate.OpenInMemory(time.Now)
	if err != nil {
		return fmt.Errorf("open store: %w", err)
	}
	defer store.Close()

	tmpDir, err := os.MkdirTemp("", "rune-fuzz-*")
	if err != nil {
		return fmt.Errorf("create temp dir: %w", err)
	}
	defer os.RemoveAll(tmpDir)

	keys := keymap.Default()
	st := styles.Default()
	reg := command.NewBuilder().Build()
	res, _ := keybind.NewResolver(nil)
	caps := terminal.TermCaps{}

	m := workspace.New(keys, st, reg, res, caps, tmpDir, nil)

	const fuzzW, fuzzH = 80, 24
	violation, frame, cells := driver.Run(m, events, store, fuzzW, fuzzH)

	if violation != nil {
		sentinel := fmt.Sprintf("<<RUNE_FUZZ_VIOLATION id=%s>>\n", violation.InvariantID)
		fmt.Print(sentinel + frame)

		if _, err := artifact.Write("artifacts", violation, frame, events, cells, fuzzW, fuzzH); err != nil {
			fmt.Fprintf(os.Stderr, "artifact write failed: %v\n", err)
		}

		// Block — Playwright takes screenshot while frozen
		select {}
	}
	return nil
}
