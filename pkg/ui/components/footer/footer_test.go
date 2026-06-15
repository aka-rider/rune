package footer

import (
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

func TestQuitChordBlockedWhenDirty(t *testing.T) {
	m := New(keymap.Default(), styles.Default())
	m = m.SetSize(80, 1)
	m = m.SetDirty(true)

	// No Text field: String() falls back to Keystroke() -> "ctrl+c",
	// matching key.WithKeys("ctrl+c") in ConfirmExitC binding.
	ctrlC := tea.KeyPressMsg{Code: 'c', Mod: tea.ModCtrl}

	// First ^C: enters pending state, shows the prompt.
	m, _ = m.Update(ctrlC)
	if !strings.Contains(m.View(), "^C") {
		t.Errorf("first ^C must show pending prompt, got:\n%s", m.View())
	}

	// Second ^C with dirty=true: must NOT quit.
	// Footer must show unsaved-changes error (bright red) instead.
	m, _ = m.Update(ctrlC)
	view := m.View()
	if !strings.Contains(strings.ToLower(view), "unsaved") {
		t.Errorf("footer must show unsaved-changes warning after second ^C, got:\n%s", view)
	}
}
