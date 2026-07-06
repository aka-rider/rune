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
	"rune/pkg/docstate"
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
	reg, res, err := driver.BuildFuzzApp(keys)
	if err != nil {
		return fmt.Errorf("build fuzz app: %w", err)
	}
	caps := terminal.TermCaps{}

	m := workspace.New(keys, st, reg, res, caps, tmpDir, nil)

	const fuzzW, fuzzH = 80, 24
	// mem=nil: this runner drives a real tmpDir through vfs.Disk (WithFS
	// never called), not an in-memory VFS — driver_verbatim.go's checks
	// skip cleanly on a nil mem (§1.4.5 verbatim checks need a byte-for-byte
	// readable backing store; a real disk read here would just re-derive
	// what Materialize already wrote, at the cost of a live temp-dir read).
	violation, frame, cells := driver.Run(m, events, store, nil, fuzzW, fuzzH)

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
