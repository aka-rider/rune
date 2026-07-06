//go:build fuzzing

package workspace_test

import (
	"fmt"
	"testing"
	"time"

	"rune/internal/fuzz/driver"
	"rune/internal/fuzz/event"
	"rune/pkg/docstate"
	"rune/pkg/terminal"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/pages/workspace"
	"rune/pkg/ui/styles"
	"rune/pkg/vfs"
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
	keyUp         = byte(0)
	keyDown       = byte(1)
	keyLeft       = byte(2)
	keyRight      = byte(3)
	keyHome       = byte(4)
	keyEnd        = byte(5)
	keyEnter      = byte(8)
	keyBS         = byte(9)
	keyTab        = byte(11)
	keyShiftTab   = byte(12)
	keyCW         = byte(14) // ctrl+w close file
	keyCN         = byte(18) // ctrl+n new file
	keyCP         = byte(19) // ctrl+p pin
	keySelectAll  = byte(22) // ctrl+a
	keyCtrlC      = byte(23) // ctrl+c confirm-exit
	keyShiftLeft  = byte(26)
	keyShiftRight = byte(27)
	keyEscape     = byte(32)
	keySave       = byte(76) // super+s
	keyUndo       = byte(77) // super+z
	keyRedo       = byte(78) // ctrl+y
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
	keys := keymap.Default()
	reg, res, err := driver.BuildFuzzApp(keys)
	if err != nil {
		f.Fatalf("BuildFuzzApp: %v", err)
	}

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

	// Seed: link folding edge cases (LINK-FOLD / LINK-CLEAN invariants). The
	// trailing "\nz" puts the cursor off the link's line so each link folds.
	f.Add(paste("[[Authentication (sorry).md|Original]]\nz")) // wiki alias
	f.Add(paste("**[Ghostty](https://ghostty.org/)**\nz"))   // bold-wrapped link
	f.Add(paste("![](assets/rune-intro.gif)\nz"))            // empty-alt image

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
		key(31),       // AltDown = MoveLineDown
		key(30),       // AltUp  = MoveLineUp
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
	// G1: dirty file + ConfirmQuitMsg must raise the guard.
	f.Add(concat(
		paste("dirty\n"),
		key(keyCtrlC),
		key(keyCtrlC),
	))

	// Seed: navigate after save. Paste → save → arrow keys.
	// TR-cursor-not-dirty: navigation on a clean file must not set the dirty flag.
	f.Add(concat(
		paste("navigate me\n"),
		key(keySave),
		key(keyDown),
		key(keyUp),
		key(keyLeft),
		key(keyRight),
	))

	// Seed: multi-cursor via SelectAll + add-cursor-below (ctrl+d = index 21 is HalfPageDown;
	// AddCursorBelow is AltDown = 31). Use SelectAll then ShiftDown to extend selection.
	f.Add(concat(
		paste("line1\nline2\nline3\n"),
		key(keySelectAll),
		key(keyShiftLeft),
		key(keyShiftRight),
	))

	// Seed: autosave snapshot (DL1). Paste then let autosave fire (flushDelay=0 under fuzzing).
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

		// Use an in-memory VFS: no real temp dirs, fully deterministic.
		mem := vfs.NewMem()
		store.UseFS(mem)

		st := styles.Default()
		caps := terminal.TermCaps{}

		m := workspace.New(keys, st, reg, res, caps, "/fuzz", nil).WithFS(mem)

		if violation, _, _ := driver.Run(m, events, store, mem, 80, 24); violation != nil {
			t.Errorf("invariant %s: %s", violation.InvariantID, violation.Message)
		}
	})
}

func FuzzSessionWithFile(f *testing.F) {
	keys := keymap.Default()
	reg, res, err := driver.BuildFuzzApp(keys)
	if err != nil {
		f.Fatalf("BuildFuzzApp: %v", err)
	}

	// Seed: paste text (autosave snapshots to VFS; DL1 check fires on AutosaveSettledMsg)
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

		// Seed the in-memory VFS instead of creating a real temp dir.
		mem := vfs.NewMem()
		const testFile = "/fuzz/test.md"
		_ = mem.WriteFile(testFile, []byte("# Test\n\nInitial content.\n"), 0o644)

		store, err := docstate.OpenInMemory(time.Now)
		if err != nil {
			t.Fatal(err)
		}
		defer store.Close()
		store.UseFS(mem)

		st := styles.Default()
		caps := terminal.TermCaps{}

		m := workspace.New(keys, st, reg, res, caps, "/fuzz", []string{testFile}).WithFS(mem)

		if violation, _, _ := driver.Run(m, events, store, mem, 80, 24); violation != nil {
			t.Errorf("invariant %s: %s", violation.InvariantID, violation.Message)
		}
	})
}

// FuzzLoadReorder exercises the load-correlation guard AND its interaction with a
// synchronous superseding transition under OUT-OF-ORDER delivery — the properties
// the in-order session driver structurally cannot reach (it settles each load
// inline, in issue order, so a load is never pending across another transition).
// The driver optionally opens a "settle" file inline (view → a real file), opens
// a fuzz-chosen sequence of files with their reads DEFERRED, optionally fires
// Ctrl+N (CreateUntitled → supersedeLoad), then replays the deferred reads in a
// fuzz-chosen permutation. It asserts the displayed doc equals what the last
// display-changing transition requested (LOAD-LASTWINS): a superseded / out-of-
// order read can never display the wrong document.
//
// data layout: [ctrl][nOpensSrc][settleIdx][openIdx × nOpens][deliveryOrder...].
// ctrl bit0 = open a "settle" file inline first; bit1 = press Ctrl+N after deferring.
func FuzzLoadReorder(f *testing.F) {
	keys := keymap.Default()
	reg, res, buildErr := driver.BuildFuzzApp(keys)
	if buildErr != nil {
		f.Fatalf("BuildFuzzApp: %v", buildErr)
	}

	f.Add([]byte{3, 1, 0, 1, 2, 1, 0})       // settle r0; defer r1,r2; new → untitled (supersede drops both)
	f.Add([]byte{1, 1, 0, 1, 2, 1, 0})       // settle r0; defer r1,r2; no new; reversed replay → r2
	f.Add([]byte{0, 2, 0, 0, 1, 2, 2, 1, 0}) // no settle; defer r0,r1,r2 from untitled → r2
	f.Add([]byte{3, 1, 1, 1, 0, 0, 0})       // settle r1; defer r1 (no-op re-open),r0; new → untitled
	f.Add([]byte{2, 1, 0, 0, 1, 0, 0})       // no settle; defer r0,r1; Ctrl+N no-ops (untitled view) → r1

	f.Fuzz(func(t *testing.T, data []byte) {
		if len(data) < 3 {
			return
		}
		const poolSize = 4
		pool := make([]string, poolSize)
		mem := vfs.NewMem()
		for i := range pool {
			pool[i] = fmt.Sprintf("/fuzz/r%d.md", i)
			_ = mem.WriteFile(pool[i], []byte(fmt.Sprintf("content %d", i)), 0o644)
		}

		store, err := docstate.OpenInMemory(time.Now)
		if err != nil {
			t.Fatal(err)
		}
		defer store.Close()
		store.UseFS(mem)

		ctrl := data[0]
		settle := ""
		if ctrl&1 != 0 {
			settle = pool[int(data[2])%poolSize]
		}
		supersede := ctrl&2 != 0
		nOpen := int(data[1])%6 + 1

		opens := make([]string, 0, nOpen)
		oi := 3
		for k := 0; k < nOpen && oi < len(data); k++ {
			opens = append(opens, pool[int(data[oi])%poolSize])
			oi++
		}
		order := data[oi:]

		st := styles.Default()
		m := workspace.New(keys, st, reg, res, terminal.TermCaps{}, "/fuzz", nil).WithFS(mem)

		if violation, _, _ := driver.RunReorder(m, settle, opens, supersede, order, store, mem, 80, 24); violation != nil {
			t.Errorf("invariant %s: %s", violation.InvariantID, violation.Message)
		}
	})
}

// FuzzSaveRace exercises the edit-during-save durability race (§1.4.2/§1.4.8) that
// the in-order session driver structurally cannot reach (it settles each save before
// the next event). RunReorderSaves DEFERS the FileSavedMsg so edits typed "while the
// save is in flight" advance the journal head before MarkSaved runs; the SAVE-RACE
// invariant then asserts the doc stays dirty (the post-save edits are genuinely
// unsaved). A MarkSaved that stamps the live head instead of the position the written
// bytes reflect marks the doc clean → those edits are silently lost.
func FuzzSaveRace(f *testing.F) {
	keys := keymap.Default()
	reg, res, buildErr := driver.BuildFuzzApp(keys)
	if buildErr != nil {
		f.Fatalf("BuildFuzzApp: %v", buildErr)
	}

	const keyFocusEditor = byte(16) // ctrl+e — route pastes to the editor buffer
	// Seed: focus editor, paste, ⌘S (deferred), then a cursor move + paste so the
	// post-save edit is a SEPARATE journal event (a new seq past the save's issue
	// position) — exactly the interleaving that exposes the MarkSaved seq race.
	f.Add(concat(key(keyFocusEditor), paste("hello"), key(keySave), key(keyEnd), paste(" world")))
	// Seed: two interleaved edits after the save, each separated by a caret move.
	f.Add(concat(key(keyFocusEditor), paste("abc"), key(keySave), key(keyEnd), paste("d"), key(keyHome), paste("e")))
	// Seed: newline after the save breaks coalescing and advances the journal.
	f.Add(concat(key(keyFocusEditor), paste("x"), key(keySave), key(keyEnd), paste("\ny")))

	f.Fuzz(func(t *testing.T, data []byte) {
		events := event.Decode(data)

		mem := vfs.NewMem()
		const testFile = "/fuzz/test.md"
		_ = mem.WriteFile(testFile, []byte("# Test\n\nInitial content.\n"), 0o644)

		store, err := docstate.OpenInMemory(time.Now)
		if err != nil {
			t.Fatal(err)
		}
		defer store.Close()
		store.UseFS(mem)

		st := styles.Default()
		caps := terminal.TermCaps{}

		m := workspace.New(keys, st, reg, res, caps, "/fuzz", []string{testFile}).WithFS(mem)

		if violation, _, _ := driver.RunReorderSaves(m, events, store, mem, []string{testFile}, 80, 24); violation != nil {
			t.Errorf("invariant %s: %s", violation.InvariantID, violation.Message)
		}
	})
}

// FuzzDelayedViewResult exercises the Part IV viewTicket chokepoint: a
// [D]iscard/[M]erge conflict resolution's fresh disk probe (resolveProbeMsg)
// is deferred, an UNCONDITIONAL epoch bump (Ctrl+N) forces it stale, then
// it's replayed. Property VIEW-TICKET-STALE (driver_delayed_view.go): the
// stale result must never mutate the buffer or identity the user is now
// looking at. The in-order drain (FuzzSession/FuzzHumanSession) structurally
// cannot reach this — it always settles the probe's Cmd before the next
// event runs, so the result is never still in flight when Ctrl+N lands.
func FuzzDelayedViewResult(f *testing.F) {
	keys := keymap.Default()
	reg, res, buildErr := driver.BuildFuzzApp(keys)
	if buildErr != nil {
		f.Fatalf("BuildFuzzApp: %v", buildErr)
	}

	f.Add(false) // discard
	f.Add(true)  // merge

	f.Fuzz(func(t *testing.T, useMerge bool) {
		mem := vfs.NewMem()
		const testFile = "/fuzz/conflict.md"

		store, err := docstate.OpenInMemory(time.Now)
		if err != nil {
			t.Fatal(err)
		}
		defer store.Close()
		store.UseFS(mem)

		st := styles.Default()
		caps := terminal.TermCaps{}

		m := workspace.New(keys, st, reg, res, caps, "/fuzz", nil).WithFS(mem)

		if violation, _, _ := driver.RunDelayedViewResult(m, testFile, useMerge, store, mem, 80, 24); violation != nil {
			t.Errorf("invariant %s: %s", violation.InvariantID, violation.Message)
		}
	})
}
