package filetree

import (
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/keymap"
	"rune/pkg/ui/listnav"
	"rune/pkg/ui/styles"
)

func newMouseTestTree(entries []Entry) Model {
	keys := keymap.Default()
	st := styles.Default()
	m := New(keys, st)
	m = m.SetSize(20, 10)
	m = m.SetFocused(true)
	m.entries = entries
	return m
}

func makeEntries(n int) []Entry {
	entries := make([]Entry, n)
	for i := range entries {
		entries[i] = Entry{Name: "file", Path: "/file"}
	}
	return entries
}

// TestMouseWheelDown verifies scroll down moves cursor by listnav.WheelLines.
func TestMouseWheelDown(t *testing.T) {
	m := newMouseTestTree(makeEntries(20))
	m.nav.Cursor = 5

	msg := tea.MouseWheelMsg{Button: tea.MouseWheelDown}
	result, _ := m.handleMouseWheel(msg)

	want := 5 + listnav.WheelLines
	if result.nav.Cursor != want {
		t.Errorf("wheel down: cursor=%d, want %d", result.nav.Cursor, want)
	}
}

// TestMouseWheelUp verifies scroll up moves cursor by listnav.WheelLines.
func TestMouseWheelUp(t *testing.T) {
	m := newMouseTestTree(makeEntries(20))
	m.nav.Cursor = 10

	msg := tea.MouseWheelMsg{Button: tea.MouseWheelUp}
	result, _ := m.handleMouseWheel(msg)

	want := 10 - listnav.WheelLines
	if result.nav.Cursor != want {
		t.Errorf("wheel up: cursor=%d, want %d", result.nav.Cursor, want)
	}
}

// TestMouseWheelDownClampedAtBottom verifies scroll down doesn't exceed last entry.
func TestMouseWheelDownClampedAtBottom(t *testing.T) {
	entries := makeEntries(5)
	m := newMouseTestTree(entries)
	m.nav.Cursor = 4 // already at last entry

	msg := tea.MouseWheelMsg{Button: tea.MouseWheelDown}
	result, _ := m.handleMouseWheel(msg)

	if result.nav.Cursor >= len(entries) {
		t.Errorf("wheel down past end: cursor=%d, len=%d", result.nav.Cursor, len(entries))
	}
	if result.nav.Cursor != 4 {
		t.Errorf("wheel down at bottom: cursor=%d, want 4", result.nav.Cursor)
	}
}

// TestMouseWheelUpClampedAtTop verifies scroll up doesn't go below 0.
func TestMouseWheelUpClampedAtTop(t *testing.T) {
	m := newMouseTestTree(makeEntries(10))
	m.nav.Cursor = 0

	msg := tea.MouseWheelMsg{Button: tea.MouseWheelUp}
	result, _ := m.handleMouseWheel(msg)

	if result.nav.Cursor < 0 {
		t.Errorf("wheel up went negative: cursor=%d", result.nav.Cursor)
	}
	if result.nav.Cursor != 0 {
		t.Errorf("wheel up at top: cursor=%d, want 0", result.nav.Cursor)
	}
}

// TestMouseClickMovesCursor verifies a click at a row sets the cursor to that entry.
func TestMouseClickMovesCursor(t *testing.T) {
	m := newMouseTestTree(makeEntries(10))
	m = m.SetOffset(0, 1) // border at y=0, content from y=1
	m.nav.Cursor = 0

	// relY = msg.Y - offsetY = 3 - 1 = 2 → entry index = start + (relY-1) = 0 + 1 = 1
	msg := tea.MouseClickMsg{Button: tea.MouseLeft, Y: 3}
	result, _ := m.handleMouseClick(msg)

	if result.nav.Cursor != 1 {
		t.Errorf("click at Y=3: cursor=%d, want 1", result.nav.Cursor)
	}
}

// TestMouseClickFirstEntry verifies clicking the first visible entry row.
func TestMouseClickFirstEntry(t *testing.T) {
	m := newMouseTestTree(makeEntries(10))
	m = m.SetOffset(0, 1)
	m.nav.Cursor = 0

	// relY = 2 - 1 = 1 → entry index = 0 + (1-1) = 0
	msg := tea.MouseClickMsg{Button: tea.MouseLeft, Y: 2}
	result, _ := m.handleMouseClick(msg)

	if result.nav.Cursor != 0 {
		t.Errorf("click at Y=2: cursor=%d, want 0", result.nav.Cursor)
	}
}

// TestMouseClickTitleRowIgnored verifies clicks on the title row are ignored.
func TestMouseClickTitleRowIgnored(t *testing.T) {
	m := newMouseTestTree(makeEntries(10))
	m = m.SetOffset(0, 1)
	m.nav.Cursor = 3

	// relY = 1 - 1 = 0 → title row, ignored
	msg := tea.MouseClickMsg{Button: tea.MouseLeft, Y: 1}
	result, _ := m.handleMouseClick(msg)

	if result.nav.Cursor != 3 {
		t.Errorf("click on title row moved cursor: cursor=%d, want 3", result.nav.Cursor)
	}
}

// TestMouseClickOutOfRangeIgnored verifies clicks beyond the entry list are ignored.
func TestMouseClickOutOfRangeIgnored(t *testing.T) {
	m := newMouseTestTree(makeEntries(3))
	m = m.SetOffset(0, 1)
	m.nav.Cursor = 0

	// relY = 10 - 1 = 9 → idx = 8, beyond len(entries)=3 → ignored
	msg := tea.MouseClickMsg{Button: tea.MouseLeft, Y: 10}
	result, _ := m.handleMouseClick(msg)

	if result.nav.Cursor != 0 {
		t.Errorf("out-of-range click moved cursor: cursor=%d, want 0", result.nav.Cursor)
	}
}

// TestMouseUnfocusedIgnoresClick verifies unfocused model ignores mouse clicks.
func TestMouseUnfocusedIgnoresClick(t *testing.T) {
	m := newMouseTestTree(makeEntries(10))
	m = m.SetOffset(0, 1)
	m = m.SetFocused(false)
	m.nav.Cursor = 5

	msg := tea.MouseClickMsg{Button: tea.MouseLeft, Y: 2}
	result, _ := m.handleMouseClick(msg)

	if result.nav.Cursor != 5 {
		t.Errorf("unfocused click moved cursor: cursor=%d, want 5", result.nav.Cursor)
	}
}

// TestMouseUnfocusedIgnoresWheel verifies unfocused model ignores mouse wheel.
func TestMouseUnfocusedIgnoresWheel(t *testing.T) {
	m := newMouseTestTree(makeEntries(10))
	m = m.SetFocused(false)
	m.nav.Cursor = 5

	msg := tea.MouseWheelMsg{Button: tea.MouseWheelDown}
	result, _ := m.handleMouseWheel(msg)

	if result.nav.Cursor != 5 {
		t.Errorf("unfocused wheel moved cursor: cursor=%d, want 5", result.nav.Cursor)
	}
}
