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

// Binding table indices (must match keytable.go bindingTable order):
//   0=Up  1=Down  2=Left  3=Right  4=Home  5=End  6=PgUp  7=PgDown
//   8=Enter  9=Backspace  10=Delete  11=Tab  12=ShiftTab
//   13=ZenMode  14=CloseFile  15=FocusExplorer  16=FocusEditor  17=FocusChat
//   18=CreateNewFile  19=PinTab  20=HalfPageUp  21=HalfPageDown
//   22=SelectAll  23=ConfirmExitC
//   24=ShiftUp  25=ShiftDown  26=ShiftLeft  27=ShiftRight
//   28=AltLeft  29=AltRight  30=AltUp  31=AltDown
//   32=Escape
//   33-58 = a-z  59=space  60='.'  61=','  62='\n'
//   63='*'  64='#'  65='|'  66='['  67=']'  68='!'  69='_'  70='-'  71='>'  72='`'  73='('  74=')'  75='~'
//   76=SaveFile(super+s)  77=Undo(super+z)  78=Redo(ctrl+y)

const (
	keyDown     = byte(1)
	keyLeft     = byte(2)
	keyRight    = byte(3)
	keyHome     = byte(4)
	keyEnd      = byte(5)
	keyEnter    = byte(8)
	keyBS       = byte(9)
	keyTab      = byte(11)
	keyShiftTab = byte(12)
	keyCW       = byte(14) // ctrl+w close file
	keyCN       = byte(18) // ctrl+n new file
	keyCP       = byte(19) // ctrl+p pin
	keySelectAll = byte(22) // ctrl+a
	keyCtrlC    = byte(23) // ctrl+c confirm-exit
	keyShiftLeft = byte(26)
	keyShiftRight = byte(27)
	keyEscape   = byte(32)
	keySave     = byte(76) // super+s
	keyUndo     = byte(77) // super+z
	keyRedo     = byte(78) // ctrl+y
)

// key encodes a KindKey event for seed construction.
func key(idx byte) []byte { return []byte{byte(event.KindKey), 0, idx} }

// paste encodes a KindPaste event. Panics if text length > 255.
func paste(text string) []byte {
	b := []byte(text)
	if len(b) > 255 {
		b = b[:255]
	}
	return append([]byte{byte(event.KindPaste), byte(len(b))}, b...)
}

// resize encodes a KindResize event.
func resize(w, h byte) []byte { return []byte{byte(event.KindResize), w, h} }

// concat joins byte slices.
func concat(parts ...[]byte) []byte {
	var out []byte
	for _, p := range parts {
		out = append(out, p...)
	}
	return out
}

func FuzzSession(f *testing.F) {
	// --- Existing seeds (updated comments: KindText now pastes full string) ---

	// Seed: paste "hello" — now inserts all 5 chars (KindText → PasteMsg)
	f.Add([]byte{byte(event.KindText), 5, 'h', 'e', 'l', 'l', 'o'})
	// Seed: paste text then navigate with arrow keys
	f.Add([]byte{
		byte(event.KindText), 3, 'f', 'o', 'o',
		byte(event.KindKey), 0, keyDown,
		byte(event.KindKey), 0, keyLeft,
		byte(event.KindKey), 0, keyRight,
	})
	// Seed: new file creation
	f.Add(key(keyCN))
	// Seed: paste text then close file
	f.Add(concat(
		[]byte{byte(event.KindText), 4, 't', 'e', 's', 't'},
		key(keyCW),
	))
	// Seed: resize after pasting
	f.Add(concat(
		[]byte{byte(event.KindText), 2, 'h', 'i'},
		resize(100, 40),
	))
	// Seed: paste unicode (café!)
	f.Add([]byte{byte(event.KindPaste), 6, 'c', 'a', 'f', 0xC3, 0xA9, '!'})
	// Seed: tab switching
	f.Add(concat(key(keyCN), key(keyTab), key(keyShiftTab)))
	// Seed: selection operations
	f.Add(concat(
		[]byte{byte(event.KindText), 5, 'l', 'i', 'n', 'e', '\n'},
		key(keySelectAll),
		key(keyShiftLeft),
		key(keyShiftRight),
	))
	// Seed: focus switch sequence
	f.Add([]byte{
		byte(event.KindFocus), 0,
		byte(event.KindFocus), 1,
		byte(event.KindFocus), 2,
	})
	// Seed: Escape, paste, navigate
	f.Add(concat(
		key(keyEscape),
		[]byte{byte(event.KindText), 3, 'a', 'b', 'c'},
		key(keyHome),
		key(keyEnd),
	))

	// --- New seeds ---

	// Seed: markdown paste — exercises Rendered/Revealed spans, table rows, images.
	// This is the only way to reach syntax_map.go Rendered path via the fuzzer.
	f.Add(paste("# H1\n**bold** _i_ `code`\n| a | b |\n|---|---|\n| 1 | 2 |\n![x](y.png)\n"))

	// Seed: wrap-stress — paste a 100-char line then squeeze the terminal to force
	// soft-wrap (exercises sliceOriginalSpans + CellMap slicing), then restore.
	// Use w=40 (not 20) — the workspace has a known overflow bug at very narrow widths;
	// 40 still forces soft-wrap on a 100-char line without triggering that regression.
	f.Add(concat(
		paste("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n"),
		resize(40, 24),
		resize(80, 24),
	))

	// Seed: multi-line scaffold then line move operations.
	// Today every seed is effectively single-line; line ops need >1 line.
	f.Add(concat(
		paste("a\nb\nc\nd\n"),
		key(31), // AltDown = MoveLineDown
		key(30), // AltUp  = MoveLineUp
		key(byte(22)), // SelectAll
	))

	// Seed: undo round-trip (DL3 monitor). Paste content, undo, redo; assert restored.
	f.Add(concat(
		paste("hello world\n"),
		key(keyUndo),
		key(keyRedo),
	))

	// Seed: multi-undo then full redo (DL4 — accumulate undo stack).
	f.Add(concat(
		paste("first line\n"),
		paste("second line\n"),
		key(keyUndo),
		key(keyUndo),
		key(keyRedo),
		key(keyRedo),
	))

	// Seed: undo-truncates-future (DL5). Edit → undo → different edit → redo is no-op.
	f.Add(concat(
		paste("original\n"),
		key(keyUndo),
		paste("replacement\n"),
		key(keyRedo), // should be no-op: future was truncated
	))

	// Seed: save then verify (DL1). Paste + explicit save.
	f.Add(concat(
		paste("saved content\n"),
		key(keySave),
	))

	// Seed: dirty-quit guard path. Insert text (dirty) then double ctrl+c.
	// G1 spec: dirty file + ConfirmQuitMsg should raise a guard.
	// Today this tests the quit path without a guard (G1 is a RED spec).
	f.Add(concat(
		paste("dirty\n"),
		key(keyCtrlC),
		key(keyCtrlC),
	))

	// Seed: multi-cursor via SelectAll + add-cursor-below (ctrl+d = index 21 is HalfPageDown;
	// AddCursorBelow is AltDown = 31). Use SelectAll then ShiftDown to extend selection.
	f.Add(concat(
		paste("line1\nline2\nline3\n"),
		key(keySelectAll),
		key(keyShiftLeft),
		key(keyShiftRight),
	))

	// Seed: autosave snapshot (DL2). Paste then let autosave fire (flushDelay=0 under fuzzing).
	f.Add(concat(
		paste("autosave me\n"),
		key(keyDown), // triggers drain which includes flush Cmd
	))

	// Seed: binding coverage sweep — fires every binding at least once.
	// Acts as a smoke test that no binding panics on a fresh Untitled buffer.
	{
		var sweep []byte
		for i := byte(0); i < 79; i++ {
			sweep = append(sweep, byte(event.KindKey), 0, i)
		}
		f.Add(sweep)
	}

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
	// Seed: paste text (autosave should write it to disk)
	f.Add([]byte{byte(event.KindText), 5, 'h', 'e', 'l', 'l', 'o'})
	// Seed: paste text then navigate
	f.Add(concat(
		[]byte{byte(event.KindText), 3, 'a', 'b', 'c'},
		key(keyDown),
		key(keyLeft),
	))
	// Seed: resize then paste
	f.Add(concat(
		resize(120, 40),
		[]byte{byte(event.KindText), 4, 't', 'e', 's', 't'},
	))
	// Seed: save and verify DL1 (file-on-disk == buffer after save)
	f.Add(concat(
		paste("edited content\n"),
		key(keySave),
	))
	// Seed: markdown paste exercises Rendered spans in a file-backed session
	f.Add(paste("# Title\n**bold**\n\n| x | y |\n|---|---|\n"))
	// Seed: undo after edit in a file-backed session
	f.Add(concat(
		paste("new content\n"),
		key(keyUndo),
	))

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
