package editor

import (
	"testing"

	"rune/pkg/command"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/keybind"
	"rune/pkg/terminal"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"

	tea "charm.land/bubbletea/v2"
)

// testEditor returns a focused editor with multi-line content and multi-cursors.
func testFindEditor(t *testing.T) Model {
	t.Helper()
	keys := keymap.Default()
	st := styles.Default()
	reg := command.NewBuilder().Build()
	res, _ := keybind.NewResolver(nil)

	m := New(keys, st, reg, res, terminal.TermCaps{})
	m = m.SetSize(80, 24)
	m = m.SetContent("test.md", []byte("hello world\nsecond line\nthird line"))
	m = m.SetFocused(true)
	// Set up multi-cursors at positions 0 and 12
	m.cursors = cursor.NewCursorSetFromPositions([]int{0, 12})
	return m
}

// openFindOverlay sends Cmd+F to open find overlay.
func openFindOverlay(m Model) Model {
	m, _ = m.Update(tea.KeyPressMsg{Code: 'f', Mod: tea.ModSuper})
	return m
}

// openReplaceFindOverlay sends Cmd+H to open find+replace overlay.
func openReplaceFindOverlay(m Model) Model {
	m, _ = m.Update(tea.KeyPressMsg{Code: 'h', Mod: tea.ModSuper})
	return m
}

func TestFindStubOpenStub(t *testing.T) {
	m := testFindEditor(t)
	contentBefore := m.Content()

	m = openFindOverlay(m)

	if !m.findOverlay.Visible() {
		t.Fatal("expected find overlay to be visible")
	}
	if m.findOverlay.replaceMode {
		t.Fatal("expected replaceMode false for find.open")
	}
	if m.Content() != contentBefore {
		t.Errorf("content mutated: got %q, want %q", m.Content(), contentBefore)
	}
}

func TestFindStubReplaceOpenStub(t *testing.T) {
	m := testFindEditor(t)
	contentBefore := m.Content()

	m = openReplaceFindOverlay(m)

	if !m.findOverlay.Visible() {
		t.Fatal("expected find overlay to be visible")
	}
	if !m.findOverlay.replaceMode {
		t.Fatal("expected replaceMode true for find.replace-open")
	}
	if m.Content() != contentBefore {
		t.Errorf("content mutated: got %q, want %q", m.Content(), contentBefore)
	}
}

func TestFindStubNextDisabled(t *testing.T) {
	m := testFindEditor(t)
	m = openFindOverlay(m)
	contentBefore := m.Content()
	cursorsBefore := m.cursors.All()

	// Cmd+G (find next) should be consumed with no effect
	m, _ = m.Update(tea.KeyPressMsg{Code: 'g', Mod: tea.ModSuper})

	if m.Content() != contentBefore {
		t.Errorf("find.next mutated content")
	}
	cursorsAfter := m.cursors.All()
	if len(cursorsAfter) != len(cursorsBefore) {
		t.Errorf("find.next changed cursor count")
	}
	for i := range cursorsBefore {
		if cursorsAfter[i].Position != cursorsBefore[i].Position {
			t.Errorf("find.next moved cursor %d", i)
		}
	}
}

func TestFindStubPrintableConsumed(t *testing.T) {
	m := testFindEditor(t)
	m = openFindOverlay(m)
	contentBefore := m.Content()

	// Type printable characters — should NOT modify buffer
	for _, ch := range []rune{'a', 'b', 'c', '1', '!'} {
		m, _ = m.Update(tea.KeyPressMsg{Code: ch})
	}

	if m.Content() != contentBefore {
		t.Errorf("printable keys mutated buffer: got %q", m.Content())
	}
}

func TestFindStubBackspaceConsumed(t *testing.T) {
	m := testFindEditor(t)
	m = openFindOverlay(m)
	contentBefore := m.Content()

	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyBackspace})

	if m.Content() != contentBefore {
		t.Errorf("backspace mutated buffer")
	}
}

func TestFindStubDeleteConsumed(t *testing.T) {
	m := testFindEditor(t)
	m = openFindOverlay(m)
	contentBefore := m.Content()

	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyDelete})

	if m.Content() != contentBefore {
		t.Errorf("delete mutated buffer")
	}
}

func TestFindStubArrowsConsumed(t *testing.T) {
	m := testFindEditor(t)
	m = openFindOverlay(m)
	cursorsBefore := m.cursors.All()

	arrows := []rune{tea.KeyLeft, tea.KeyRight, tea.KeyUp, tea.KeyDown}
	for _, code := range arrows {
		m, _ = m.Update(tea.KeyPressMsg{Code: code})
	}

	cursorsAfter := m.cursors.All()
	if len(cursorsAfter) != len(cursorsBefore) {
		t.Fatalf("arrows changed cursor count: %d → %d", len(cursorsBefore), len(cursorsAfter))
	}
	for i := range cursorsBefore {
		if cursorsAfter[i].Position != cursorsBefore[i].Position {
			t.Errorf("arrow moved cursor %d: %d → %d", i, cursorsBefore[i].Position, cursorsAfter[i].Position)
		}
	}
}

func TestFindStubEnterDisabled(t *testing.T) {
	m := testFindEditor(t)
	m = openFindOverlay(m)
	contentBefore := m.Content()

	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})

	if m.Content() != contentBefore {
		t.Errorf("enter mutated buffer")
	}
}

func TestFindStubShiftEnterDisabled(t *testing.T) {
	m := testFindEditor(t)
	m = openFindOverlay(m)
	contentBefore := m.Content()

	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEnter, Mod: tea.ModShift})

	if m.Content() != contentBefore {
		t.Errorf("shift+enter mutated buffer")
	}
}

func TestFindStubUndoRedoConsumed(t *testing.T) {
	m := testFindEditor(t)
	m = openFindOverlay(m)
	canUndoBefore := m.history.CanUndo()
	canRedoBefore := m.history.CanRedo()

	// Cmd+Z (undo)
	m, _ = m.Update(tea.KeyPressMsg{Code: 'z', Mod: tea.ModSuper})
	// Cmd+Shift+Z (redo)
	m, _ = m.Update(tea.KeyPressMsg{Code: 'z', Mod: tea.ModSuper | tea.ModShift})

	if m.history.CanUndo() != canUndoBefore {
		t.Errorf("undo state changed while overlay open")
	}
	if m.history.CanRedo() != canRedoBefore {
		t.Errorf("redo state changed while overlay open")
	}
}

func TestFindStubClipboardConsumed(t *testing.T) {
	m := testFindEditor(t)
	m = openFindOverlay(m)
	contentBefore := m.Content()

	// Cmd+V (paste), Cmd+C (copy), Cmd+X (cut) — all consumed
	m, _ = m.Update(tea.KeyPressMsg{Code: 'v', Mod: tea.ModSuper})
	m, _ = m.Update(tea.KeyPressMsg{Code: 'c', Mod: tea.ModSuper})
	m, _ = m.Update(tea.KeyPressMsg{Code: 'x', Mod: tea.ModSuper})

	if m.Content() != contentBefore {
		t.Errorf("clipboard keys mutated buffer")
	}
}

func TestFindStubSelectAllConsumed(t *testing.T) {
	m := testFindEditor(t)
	m = openFindOverlay(m)
	cursorsBefore := m.cursors.All()

	// Cmd+A (select all)
	m, _ = m.Update(tea.KeyPressMsg{Code: 'a', Mod: tea.ModSuper})

	cursorsAfter := m.cursors.All()
	if len(cursorsAfter) != len(cursorsBefore) {
		t.Errorf("select-all changed cursor count")
	}
	for i := range cursorsBefore {
		if cursorsAfter[i] != cursorsBefore[i] {
			t.Errorf("select-all changed cursor %d", i)
		}
	}
}

func TestFindStubWordDeleteConsumed(t *testing.T) {
	m := testFindEditor(t)
	m = openFindOverlay(m)
	contentBefore := m.Content()

	// Option+Backspace (word delete)
	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyBackspace, Mod: tea.ModAlt})
	// Option+Delete (word forward delete)
	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyDelete, Mod: tea.ModAlt})

	if m.Content() != contentBefore {
		t.Errorf("word-delete mutated buffer")
	}
}

func TestFindStubTabConsumed(t *testing.T) {
	m := testFindEditor(t)
	m = openFindOverlay(m)
	contentBefore := m.Content()
	cursorsBefore := m.cursors.All()
	canUndoBefore := m.history.CanUndo()

	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyTab})

	if m.Content() != contentBefore {
		t.Errorf("tab mutated content")
	}
	cursorsAfter := m.cursors.All()
	for i := range cursorsBefore {
		if cursorsAfter[i].Position != cursorsBefore[i].Position {
			t.Errorf("tab moved cursor %d", i)
		}
	}
	if m.history.CanUndo() != canUndoBefore {
		t.Errorf("tab changed history")
	}
}

func TestFindStubEscapePriority(t *testing.T) {
	m := testFindEditor(t)
	// Ensure multi-cursors
	if m.cursors.Len() < 2 {
		t.Fatal("test setup: expected multi-cursors")
	}
	m = openFindOverlay(m)
	if !m.findOverlay.Visible() {
		t.Fatal("overlay should be visible")
	}

	// Press Escape — should close overlay, NOT collapse multi-cursors
	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEscape})

	if m.findOverlay.Visible() {
		t.Errorf("escape did not close overlay")
	}
	if m.cursors.Len() < 2 {
		t.Errorf("escape collapsed multi-cursors: got %d, want >=2", m.cursors.Len())
	}
}
