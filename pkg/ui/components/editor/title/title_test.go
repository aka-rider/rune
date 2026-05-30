package title

import (
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/styles"
)

func newTestTitle() Model {
	return New("Untitled 1", styles.Default())
}

func TestTitle_DefaultText(t *testing.T) {
	m := newTestTitle()
	if m.Text() != "Untitled 1" {
		t.Errorf("expected 'Untitled 1', got %q", m.Text())
	}
	if !m.IsPlaceholder() {
		t.Error("expected IsPlaceholder to be true")
	}
}

func TestTitle_SetText(t *testing.T) {
	m := newTestTitle()
	m = m.SetText("my-note")
	if m.Text() != "my-note" {
		t.Errorf("expected 'my-note', got %q", m.Text())
	}
	if m.IsPlaceholder() {
		t.Error("expected IsPlaceholder to be false after SetText")
	}
}

func TestTitle_TypeChar(t *testing.T) {
	m := newTestTitle()
	m = m.SetFocused(true)
	// Clear default text first via select-all equivalent (multiple backspaces)
	m.text = ""
	m.cursor = 0

	m, _ = m.Update(tea.KeyPressMsg{Text: "h"})
	m, _ = m.Update(tea.KeyPressMsg{Text: "i"})
	if m.text != "hi" {
		t.Errorf("expected 'hi', got %q", m.text)
	}
	if m.cursor != 2 {
		t.Errorf("expected cursor=2, got %d", m.cursor)
	}
}

func TestTitle_Backspace(t *testing.T) {
	m := newTestTitle()
	m = m.SetFocused(true)
	m.text = "abc"
	m.cursor = 3

	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyBackspace})
	if m.text != "ab" {
		t.Errorf("expected 'ab', got %q", m.text)
	}
}

func TestTitle_CursorMovement(t *testing.T) {
	m := newTestTitle()
	m = m.SetFocused(true)
	m.text = "hello"
	m.cursor = 5

	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyLeft})
	if m.cursor != 4 {
		t.Errorf("expected cursor=4, got %d", m.cursor)
	}

	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyHome})
	if m.cursor != 0 {
		t.Errorf("expected cursor=0, got %d", m.cursor)
	}

	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEnd})
	if m.cursor != 5 {
		t.Errorf("expected cursor=5, got %d", m.cursor)
	}
}

func TestTitle_DownReturnsFocus(t *testing.T) {
	m := newTestTitle()
	m = m.SetFocused(true)

	m, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyDown})
	if m.focused {
		t.Error("expected focused=false after Down")
	}
	if cmd == nil {
		t.Fatal("expected a Cmd")
	}
	result := cmd()
	if _, ok := result.(FocusReturnMsg); !ok {
		t.Fatalf("expected FocusReturnMsg, got %T", result)
	}
}

func TestTitle_EscapeReverts(t *testing.T) {
	m := newTestTitle()
	m = m.SetText("committed-name")
	m = m.SetFocused(true)
	m.text = "changed"

	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEscape})
	if m.text != "committed-name" {
		t.Errorf("expected revert to 'committed-name', got %q", m.text)
	}
	if m.focused {
		t.Error("expected focused=false after Escape")
	}
}

func TestTitle_DebounceEmitsRename(t *testing.T) {
	m := newTestTitle()
	m = m.SetText("original")
	m = m.SetFocused(true)
	m.text = "original"
	m.cursor = 8

	// Type a character to trigger debounce
	m, cmd := m.Update(tea.KeyPressMsg{Text: "x"})
	if cmd == nil {
		t.Fatal("expected debounce Cmd")
	}

	// Execute the timer (simulates 500ms passing)
	result := cmd()
	expiredMsg := result.(debounceExpiredMsg)

	// Feed the expired msg back
	m, cmd = m.Update(expiredMsg)
	if cmd == nil {
		t.Fatal("expected RenameRequestMsg Cmd")
	}
	renameResult := cmd()
	rename, ok := renameResult.(RenameRequestMsg)
	if !ok {
		t.Fatalf("expected RenameRequestMsg, got %T", renameResult)
	}
	if rename.Name != "originalx" {
		t.Errorf("expected Name='originalx', got %q", rename.Name)
	}
}

func TestTitle_IgnoredWhenNotFocused(t *testing.T) {
	m := newTestTitle()
	// Not focused — keys should be ignored
	m, cmd := m.Update(tea.KeyPressMsg{Text: "x"})
	if cmd != nil {
		t.Error("expected nil Cmd when not focused")
	}
	if m.text != "Untitled 1" {
		t.Errorf("expected text unchanged, got %q", m.text)
	}
}

func TestTitle_View(t *testing.T) {
	m := newTestTitle()
	m = m.SetSize(80, 1)
	view := m.View()
	if view == "" {
		t.Error("expected non-empty view")
	}
}
