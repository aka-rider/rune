package footer

import (
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// TestQuitChordEmitsConfirmQuitMsg verifies that the footer always emits
// ConfirmQuitMsg on the second ^C press. Dirty-state handling (guard prompt)
// is the workspace's responsibility, not the footer's.
func TestQuitChordEmitsConfirmQuitMsg(t *testing.T) {
	m := New(keymap.Default(), styles.Default())
	m = m.SetSize(80, 1)

	// No Text field: String() falls back to Keystroke() -> "ctrl+c",
	// matching key.WithKeys("ctrl+c") in ConfirmExitC binding.
	ctrlC := tea.KeyPressMsg{Code: 'c', Mod: tea.ModCtrl}

	// First ^C: enters pending state; no quit yet.
	m, cmd := m.Update(ctrlC)
	if cmd != nil {
		if msg := cmd(); msg != nil {
			if _, isQuit := msg.(ConfirmQuitMsg); isQuit {
				t.Fatal("first ^C must not emit ConfirmQuitMsg")
			}
		}
	}

	// Second ^C: must emit ConfirmQuitMsg.
	_, cmd = m.Update(ctrlC)
	if cmd == nil {
		t.Fatal("second ^C must return a non-nil Cmd")
	}
	msg := cmd()
	if _, ok := msg.(ConfirmQuitMsg); !ok {
		t.Fatalf("second ^C must emit ConfirmQuitMsg, got %T", msg)
	}
}
