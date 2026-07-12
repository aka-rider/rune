package workspace_test

// First REAL-INPUT coverage for multi-cursor editing (the R1–R9 cell and
// C1–C3 cursor invariants previously only ever saw multi-cursor state through
// the fuzz driver's synthetic events): add-cursor-below twice via the real
// ⌥⌘↓ keys, type one character through the real key path (inserted at all
// three cursors as ONE journal stop), undo/redo via real keys, Escape
// collapses. invarianttest.CheckWorkspace runs the fuzzer's full invariant
// sweep after EVERY key — this file is package workspace_test precisely so it
// may import invarianttest (which imports workspace; an internal test file
// would close an import cycle).

import (
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"

	"rune/internal/editortest"
	"rune/internal/fuzz/driver"
	"rune/internal/fuzz/invarianttest"
	"rune/pkg/docstate"
	"rune/pkg/terminal"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/pages/workspace"
	"rune/pkg/ui/styles"
	"rune/pkg/vfs"
)

func TestMultiCursor_RealKeys_OneJournalStopPerUndo(t *testing.T) {
	keys := keymap.Default()
	reg, res, err := driver.BuildFuzzApp(keys)
	if err != nil {
		t.Fatalf("BuildFuzzApp: %v", err)
	}

	const path = "/docs/mc.md"
	const original = "aaa\nbbb\nccc\n"
	mem := vfs.NewMem()
	if err := mem.WriteFile(path, []byte(original), 0o644); err != nil {
		t.Fatal(err)
	}
	store, err := docstate.OpenInMemory(editortest.AutoClock(time.Millisecond))
	if err != nil {
		t.Fatal(err)
	}
	defer store.Close()
	store.UseFS(mem)

	m := workspace.New(keys, styles.Default(), reg, res, terminal.TermCaps{}, "/docs", []string{path}).
		WithFS(mem).WithWatcher(workspace.NoopWatcher{})

	// step delivers one message, settles the round trip, and runs the full
	// invariant sweep — the same session.Check the fuzz driver applies.
	step := func(m workspace.Model, msg tea.Msg) workspace.Model {
		t.Helper()
		m, cmd := m.Update(msg)
		m = editortest.Drain(m, cmd, workspace.Model.Update)
		invarianttest.CheckWorkspace(t, m)
		return m
	}
	key := func(m workspace.Model, code rune, mod tea.KeyMod, text string) workspace.Model {
		t.Helper()
		return step(m, tea.KeyPressMsg{Code: code, Mod: mod, Text: text})
	}

	m = step(m, tea.WindowSizeMsg{Width: 80, Height: 24})
	m = editortest.Drain(m, m.Init(), workspace.Model.Update)
	m = step(m, workspace.StoreReadyMsg{Store: store})

	docID := m.FuzzInspect().DocID
	if docID == 0 {
		t.Fatal("setup: file did not resolve to a real doc")
	}
	if got := m.FuzzInspect().Content; got != original {
		t.Fatalf("setup: content = %q, want %q", got, original)
	}

	// Focus the editor (^e) — a fresh workspace focuses the file tree.
	m = key(m, 'e', tea.ModCtrl, "")
	if !m.FuzzInspect().Focused {
		t.Fatal("setup: editor not focused after ctrl+e")
	}

	// ⌥⌘↓ twice: three cursors, one per line, same column.
	m = step(m, tea.KeyPressMsg{Code: tea.KeyDown, Mod: tea.ModAlt | tea.ModSuper})
	m = step(m, tea.KeyPressMsg{Code: tea.KeyDown, Mod: tea.ModAlt | tea.ModSuper})
	if got := len(m.FuzzInspect().CursorOffsets); got != 3 {
		t.Fatalf("after ⌥⌘↓ ×2: %d cursors, want 3", got)
	}

	seqBefore, err := store.CurrentSeq(docID)
	if err != nil {
		t.Fatalf("CurrentSeq before typing: %v", err)
	}

	// One real keystroke inserts at ALL three cursors...
	m = key(m, 'x', 0, "x")
	const edited = "xaaa\nxbbb\nxccc\n"
	if got := m.FuzzInspect().Content; got != edited {
		t.Fatalf("multi-cursor insert: content = %q, want %q", got, edited)
	}
	// ...as exactly ONE journal stop.
	seqAfter, err := store.CurrentSeq(docID)
	if err != nil {
		t.Fatalf("CurrentSeq after typing: %v", err)
	}
	if seqAfter != seqBefore+1 {
		t.Fatalf("multi-cursor insert journaled %d stops, want exactly 1 (seq %d → %d)",
			seqAfter-seqBefore, seqBefore, seqAfter)
	}

	// ⌘Z reverts all three insertions in one step.
	m = key(m, 'z', tea.ModSuper, "")
	if got := m.FuzzInspect().Content; got != original {
		t.Fatalf("⌘Z: content = %q, want all three cursors reverted to %q", got, original)
	}

	// Redo (⇧⌘Z) re-applies all three in one step.
	m = key(m, 'z', tea.ModShift|tea.ModSuper, "")
	if got := m.FuzzInspect().Content; got != edited {
		t.Fatalf("redo: content = %q, want %q", got, edited)
	}

	// Escape collapses back to a single cursor.
	m = step(m, tea.KeyPressMsg{Code: tea.KeyEscape})
	if got := len(m.FuzzInspect().CursorOffsets); got != 1 {
		t.Fatalf("Escape: %d cursors, want collapsed to 1", got)
	}
}
