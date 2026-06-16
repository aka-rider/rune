//go:build fuzzing

package workspace_test

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	"rune/internal/fuzz/driver"
	"rune/internal/fuzz/event"
	"rune/pkg/command"
	"rune/pkg/docstate"
	"rune/pkg/editor/keybind"
	"rune/pkg/terminal"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/pages/workspace"
	"rune/pkg/ui/styles"
)

func FuzzSession(f *testing.F) {
	// Seed: multi-char text insertion ("hello")
	f.Add([]byte{byte(event.KindText), 5, 'h', 'e', 'l', 'l', 'o'})
	// Seed: type text then navigate with arrow keys (Down, Left, Right)
	f.Add([]byte{
		byte(event.KindText), 3, 'f', 'o', 'o',
		byte(event.KindKey), 0, 1, // Down
		byte(event.KindKey), 0, 2, // Left
		byte(event.KindKey), 0, 3, // Right
	})
	// Seed: new file creation (ctrl+n = index 18)
	f.Add([]byte{byte(event.KindKey), 0, 18})
	// Seed: type text then close file (ctrl+w = index 14)
	f.Add([]byte{
		byte(event.KindText), 4, 't', 'e', 's', 't',
		byte(event.KindKey), 0, 14,
	})
	// Seed: resize event after typing
	f.Add([]byte{
		byte(event.KindText), 2, 'h', 'i',
		byte(event.KindResize), 100, 40,
	})
	// Seed: paste event with unicode (café)
	f.Add([]byte{byte(event.KindPaste), 6, 'c', 'a', 'f', 0xC3, 0xA9, '!'})
	// Seed: tab switching sequence (ShiftTab = index 12, Tab = index 11)
	f.Add([]byte{
		byte(event.KindKey), 0, 18, // ctrl+n  — create a file
		byte(event.KindKey), 0, 11, // Tab
		byte(event.KindKey), 0, 12, // ShiftTab
	})
	// Seed: multiple cursor operations (SelectAll = index 22, then arrow keys)
	f.Add([]byte{
		byte(event.KindText), 5, 'l', 'i', 'n', 'e', '\n',
		byte(event.KindKey), 0, 22, // ctrl+a (SelectAll)
		byte(event.KindKey), 0, 26, // ShiftLeft
		byte(event.KindKey), 0, 27, // ShiftRight
	})
	// Seed: focus switch sequence (FocusExplorer→FocusEditor→FocusChat)
	f.Add([]byte{
		byte(event.KindFocus), 0, // FocusExplorer
		byte(event.KindFocus), 1, // FocusEditor
		byte(event.KindFocus), 2, // FocusChat
	})
	// Seed: Escape to cancel any pending state, then type and navigate
	f.Add([]byte{
		byte(event.KindKey), 0, 32, // Escape
		byte(event.KindText), 3, 'a', 'b', 'c',
		byte(event.KindKey), 0, 4, // Home
		byte(event.KindKey), 0, 5, // End
	})

	f.Fuzz(func(t *testing.T, data []byte) {
		events := event.Decode(data)

		store, err := docstate.OpenInMemory(time.Now)
		if err != nil {
			t.Fatal(err)
		}
		defer store.Close()

		tmpDir, err := os.MkdirTemp("", "rune-fuzz-*")
		if err != nil {
			t.Fatal(err)
		}
		defer os.RemoveAll(tmpDir)

		keys := keymap.Default()
		st := styles.Default()
		reg := command.NewBuilder().Build()
		res, _ := keybind.NewResolver(nil)
		caps := terminal.TermCaps{}

		m := workspace.New(keys, st, reg, res, caps, tmpDir, nil)

		if violation, _, _ := driver.Run(m, events, store, 80, 24); violation != nil {
			t.Errorf("invariant %s: %s", violation.InvariantID, violation.Message)
		}
	})
}

func FuzzSessionWithFile(f *testing.F) {
	// Seed: type text (autosave should write it to disk)
	f.Add([]byte{byte(event.KindText), 5, 'h', 'e', 'l', 'l', 'o'})
	// Seed: type text then navigate
	f.Add([]byte{
		byte(event.KindText), 3, 'a', 'b', 'c',
		byte(event.KindKey), 0, 1, // Down
		byte(event.KindKey), 0, 2, // Left
	})
	// Seed: resize then type
	f.Add([]byte{
		byte(event.KindResize), 120, 40,
		byte(event.KindText), 4, 't', 'e', 's', 't',
	})

	f.Fuzz(func(t *testing.T, data []byte) {
		events := event.Decode(data)

		tmpDir := t.TempDir()
		// pre-create a markdown file so filePath is set in the workspace
		testFile := filepath.Join(tmpDir, "test.md")
		if err := os.WriteFile(testFile, []byte("# Test\n\nInitial content.\n"), 0644); err != nil {
			t.Fatal(err)
		}

		store, _, err := docstate.OpenAt(tmpDir)
		if err != nil {
			t.Fatal(err)
		}
		defer store.Close()

		keys := keymap.Default()
		st := styles.Default()
		reg := command.NewBuilder().Build()
		res, _ := keybind.NewResolver(nil)
		caps := terminal.TermCaps{}

		m := workspace.New(keys, st, reg, res, caps, tmpDir, []string{testFile})

		if violation, _, _ := driver.Run(m, events, store, 80, 24); violation != nil {
			t.Errorf("invariant %s: %s", violation.InvariantID, violation.Message)
		}
	})
}
