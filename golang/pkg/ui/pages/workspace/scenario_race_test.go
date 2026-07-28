package workspace_test

// Phase 2(c) of the QA-rehaul plan: deterministic, fixed-permutation
// re-checks of the three race drivers (RunReorder, RunReorderSaves,
// RunDelayedViewResult) that FuzzLoadReorder/FuzzSaveRace/
// FuzzDelayedViewResult (session_fuzz_test.go) fuzz — same driver entry
// points, same invariants, fixed inputs instead of a fuzz-chosen byte
// stream, so `make test` deterministically re-checks the reordering
// properties on every run.
//
// External (package workspace_test) for the same reason as scenario_test.go:
// it imports rune/internal/fuzz/driver, which imports
// rune/pkg/ui/pages/workspace.

import (
	"fmt"
	"testing"
	"time"

	"rune/internal/editortest"
	"rune/internal/fuzz/driver"
	"rune/internal/fuzz/event"
	"rune/pkg/docstate"
	"rune/pkg/terminal"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/pages/workspace"
	"rune/pkg/ui/styles"
	"rune/pkg/vfs"
)

// TestLoadReorder re-checks LOAD-LASTWINS (driver.go) under three fixed
// delivery permutations of three deferred file-load reads, crossed with
// supersede on/off:
//
//   - identity: reads land in issue order (opens[0], opens[1], opens[2]).
//   - full-reverse: reads land in reverse issue order.
//   - rotate-by-1: reads land as opens[1], opens[2], opens[0].
//
// order's bytes are consumed mod len(remaining-deferred) each step
// (driver.go's RunReorder replay loop): {0,0,0} always takes the front of
// the shrinking list (identity); {2,1,0} always takes the back (full
// reverse); {1,1,0} takes the middle first, then the (now-front) last
// original element, then what's left (rotate-by-1). Verified against
// RunReorder's replay loop shape, not just asserted by name.
func TestLoadReorder(t *testing.T) {
	permutations := []struct {
		name  string
		order []byte
	}{
		{"identity", []byte{0, 0, 0}},
		{"fullReverse", []byte{2, 1, 0}},
		{"rotateBy1", []byte{1, 1, 0}},
	}

	keys := keymap.Default()
	reg, res, err := driver.BuildFuzzApp(keys)
	if err != nil {
		t.Fatalf("BuildFuzzApp: %v", err)
	}

	for _, perm := range permutations {
		for _, supersede := range []bool{false, true} {
			name := fmt.Sprintf("%s/supersede=%v", perm.name, supersede)
			t.Run(name, func(t *testing.T) {
				t.Parallel()

				const poolSize = 4
				pool := make([]string, poolSize)
				mem := vfs.NewMem()
				for i := range pool {
					pool[i] = fmt.Sprintf("/fuzz/r%d.md", i)
					_ = mem.WriteFile(pool[i], fmt.Appendf(nil, "content %d", i), 0o644)
				}

				store, err := docstate.OpenInMemory(editortest.AutoClock(time.Millisecond))
				if err != nil {
					t.Fatal(err)
				}
				defer store.Close()
				store.UseFS(mem)

				opens := []string{pool[0], pool[1], pool[2]}

				st := styles.Default()
				m := workspace.New(keys, st, reg, res, terminal.TermCaps{}, "/fuzz", nil).WithFS(mem).WithWatcher(workspace.NoopWatcher{})

				if violation, _, _ := driver.RunReorder(m, "", opens, supersede, perm.order, store, mem, 80, 24); violation != nil {
					t.Fatalf("invariant %s: %s", violation.InvariantID, violation.Message)
				}
			})
		}
	}
}

// TestSaveRace re-checks SAVE-RACE (§1.4.2/§1.4.8) via RunReorderSaves using
// FuzzSaveRace's three named seeds — a save-in-flight followed by post-save
// edits must leave the doc dirty (the post-save edits are genuinely
// unsaved).
func TestSaveRace(t *testing.T) {
	const keyFocusEditor = byte(16) // ctrl+e -- route pastes to the editor buffer

	seeds := []struct {
		name string
		data []byte
	}{
		{"separateJournalEvent", concat(key(keyFocusEditor), paste("hello"), key(keySave), key(keyEnd), paste(" world"))},
		{"twoInterleavedEdits", concat(key(keyFocusEditor), paste("abc"), key(keySave), key(keyEnd), paste("d"), key(keyHome), paste("e"))},
		{"newlineBreaksCoalescing", concat(key(keyFocusEditor), paste("x"), key(keySave), key(keyEnd), paste("\ny"))},
	}

	keys := keymap.Default()
	reg, res, err := driver.BuildFuzzApp(keys)
	if err != nil {
		t.Fatalf("BuildFuzzApp: %v", err)
	}

	for _, seed := range seeds {
		t.Run(seed.name, func(t *testing.T) {
			t.Parallel()

			events := event.Decode(seed.data)

			mem := vfs.NewMem()
			const testFile = "/fuzz/test.md"
			_ = mem.WriteFile(testFile, []byte("# Test\n\nInitial content.\n"), 0o644)

			store, err := docstate.OpenInMemory(editortest.AutoClock(time.Millisecond))
			if err != nil {
				t.Fatal(err)
			}
			defer store.Close()
			store.UseFS(mem)

			st := styles.Default()
			caps := terminal.TermCaps{}

			m := workspace.New(keys, st, reg, res, caps, "/fuzz", []string{testFile}).WithFS(mem).WithWatcher(workspace.NoopWatcher{})

			if violation, _, _ := driver.RunReorderSaves(m, events, store, mem, []string{testFile}, 80, 24); violation != nil {
				t.Fatalf("invariant %s: %s", violation.InvariantID, violation.Message)
			}
		})
	}
}

// TestDelayedViewResult re-checks VIEW-TICKET-STALE (driver_delayed_view.go)
// with a deferred conflict-resolution probe forced stale by an unconditional
// epoch bump, for both the discard and merge resolution paths.
func TestDelayedViewResult(t *testing.T) {
	keys := keymap.Default()
	reg, res, err := driver.BuildFuzzApp(keys)
	if err != nil {
		t.Fatalf("BuildFuzzApp: %v", err)
	}

	for _, useMerge := range []bool{false, true} {
		name := "discard"
		if useMerge {
			name = "merge"
		}
		t.Run(name, func(t *testing.T) {
			t.Parallel()

			mem := vfs.NewMem()
			const testFile = "/fuzz/conflict.md"

			store, err := docstate.OpenInMemory(editortest.AutoClock(time.Millisecond))
			if err != nil {
				t.Fatal(err)
			}
			defer store.Close()
			store.UseFS(mem)

			st := styles.Default()
			caps := terminal.TermCaps{}

			m := workspace.New(keys, st, reg, res, caps, "/fuzz", nil).WithFS(mem).WithWatcher(workspace.NoopWatcher{})

			if violation, _, _ := driver.RunDelayedViewResult(m, testFile, useMerge, store, mem, 80, 24); violation != nil {
				t.Fatalf("invariant %s: %s", violation.InvariantID, violation.Message)
			}
		})
	}
}
