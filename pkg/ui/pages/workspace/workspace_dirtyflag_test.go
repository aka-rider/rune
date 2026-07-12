package workspace

import (
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/components/footer"
)

// ─────────────────────────────────────────────────────────────────────────────
// Dirty flag: edit sets it, save clears it, quit guard fires when dirty
// ─────────────────────────────────────────────────────────────────────────────

func TestDirtyFlagSetOnEdit(t *testing.T) {
	m := newTestWorkspace(t)
	m = withStore(t, m)
	m = loadFile(m, "note.md", "hello")
	m = focusEditor(m)

	if m.opentabs.HasDirty() {
		t.Fatal("tab should not be dirty after file load")
	}

	// Type a character — buffer revision advances, dirty flag must be set.
	m, _ = m.Update(tea.KeyPressMsg{Code: 'a'})

	if !m.opentabs.HasDirty() {
		t.Fatal("tab must be dirty after edit")
	}
}

func TestDirtyFlagClearedOnSave(t *testing.T) {
	m := newTestWorkspace(t)
	m = withStore(t, m)
	m = loadFile(m, "note.md", "hello")
	m = focusEditor(m)

	// Edit to make dirty.
	m, _ = m.Update(tea.KeyPressMsg{Code: 'a'})
	if !m.opentabs.HasDirty() {
		t.Fatal("tab must be dirty after edit")
	}

	// Start a save and drain the real materializeStoreCmd round trip, exactly
	// as production does.
	var saveCmd tea.Cmd
	m, saveCmd = m.startSave()
	m = settle(t, m, saveCmd)

	if m.opentabs.HasDirty() {
		t.Fatal("tab must be clean after save")
	}
}

func TestQuitGuardAppearsWhenDirty(t *testing.T) {
	m := newTestWorkspace(t)
	m = withStore(t, m)
	m = loadFile(m, "note.md", "hello")
	m = focusEditor(m)

	// Edit to make dirty.
	m, _ = m.Update(tea.KeyPressMsg{Code: 'a'})
	if !m.opentabs.HasDirty() {
		t.Fatal("tab must be dirty after edit (prerequisite)")
	}

	// ConfirmQuitMsg must raise the guard, not quit.
	m, cmd := m.Update(footer.ConfirmQuitMsg{})
	if cmd != nil {
		// The returned cmd must not be tea.Quit — execute it and check.
		result := execCmds(cmd)
		for _, msg := range result {
			if _, isQuit := msg.(tea.QuitMsg); isQuit {
				t.Fatal("workspace quit immediately with unsaved changes — guard not raised")
			}
		}
	}
	if !m.footer.InGuard() {
		t.Fatal("footer guard must be visible after ConfirmQuitMsg with dirty file")
	}
}

func TestCursorMovementDoesNotSetDirty(t *testing.T) {
	m := newTestWorkspace(t)
	m = loadFile(m, "note.md", "hello world")
	m = focusEditor(m)

	if m.opentabs.HasDirty() {
		t.Fatal("tab should not be dirty after file load")
	}

	navKeys := []tea.KeyPressMsg{
		{Code: tea.KeyRight},
		{Code: tea.KeyLeft},
		{Code: tea.KeyDown},
		{Code: tea.KeyUp},
		{Code: tea.KeyEnd},
		{Code: tea.KeyHome},
	}
	for _, k := range navKeys {
		m, _ = m.Update(k)
		if m.opentabs.HasDirty() {
			t.Fatalf("cursor movement (%v) must not set dirty flag", k)
		}
	}
}

func TestQuitProceedsWhenClean(t *testing.T) {
	m := newTestWorkspace(t)
	m = loadFile(m, "note.md", "hello")

	// No edits — ConfirmQuitMsg must not raise a guard (proceed directly to quit).
	m, cmd := m.Update(footer.ConfirmQuitMsg{})
	if m.footer.InGuard() {
		t.Fatal("guard must not appear when file is clean")
	}
	// A non-nil cmd means the quit sequence was initiated.
	if cmd == nil {
		t.Fatal("expected a quit cmd for clean file")
	}
}
