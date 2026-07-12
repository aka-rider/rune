package opentabs

import (
	"fmt"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

func TestMouseClickSelectsTab(t *testing.T) {
	m := New(keymap.Default(), styles.Default())
	m = m.OpenFile(1, "a.md").OpenFile(2, "b.md") // tabs: a(0), b(1)
	m = m.SetFocused(true).SetOffset(1, 5)        // header at y=5, tab0 y=6, tab1 y=7

	// Click tab 1 (b.md).
	_, cmd := m.Update(tea.MouseClickMsg{X: 3, Y: 7, Button: tea.MouseLeft})
	if cmd == nil {
		t.Fatal("expected a TabSelectedMsg cmd from clicking a tab")
	}
	sel, ok := cmd().(TabSelectedMsg)
	if !ok {
		t.Fatalf("expected TabSelectedMsg, got %T", cmd())
	}
	if sel.Path != "b.md" {
		t.Fatalf("expected b.md, got %q", sel.Path)
	}

	// Click the header row → no selection.
	if _, c := m.Update(tea.MouseClickMsg{X: 3, Y: 5, Button: tea.MouseLeft}); c != nil {
		t.Fatal("clicking the header row must not select a tab")
	}
	// Click past the last tab → no selection.
	if _, c := m.Update(tea.MouseClickMsg{X: 3, Y: 20, Button: tea.MouseLeft}); c != nil {
		t.Fatal("clicking past the last tab must not select a tab")
	}
}

func TestDirtyFlagPosition(t *testing.T) {
	m := New(keymap.Default(), styles.Default())
	m = m.SetSize(60, 10)
	m = m.OpenFile(1, "/notes/tickets.txt")

	// Clean: no 'x' appears before the filename in the rendered view.
	view := m.View()
	nameIdx := strings.Index(view, "tickets.txt")
	if nameIdx < 0 {
		t.Fatalf("expected 'tickets.txt' in clean view, got:\n%s", view)
	}
	if strings.Contains(view[:nameIdx], "x") {
		t.Errorf("clean tab must not contain 'x' before the filename, got:\n%s", view)
	}

	// Dirty: 'x' must appear before the filename (in the number-prefix slot).
	m = m.SetDirty(TabHandle{DocID: 1}, true)
	view = m.View()
	nameIdx = strings.Index(view, "tickets.txt")
	if nameIdx < 0 {
		t.Fatalf("expected 'tickets.txt' in dirty view, got:\n%s", view)
	}
	if !strings.Contains(view[:nameIdx], "x") {
		t.Errorf("dirty tab must have 'x' before the filename, got:\n%s", view)
	}
	// Dirty marker must NOT appear after the filename.
	afterName := view[nameIdx+len("tickets.txt"):]
	if strings.Contains(afterName, "x") || strings.Contains(afterName, "●") {
		t.Errorf("dirty marker must not appear after the filename, got:\n%s", view)
	}
}

// TestView_CursorStaysVisibleUnderOverflow pins the B4 windowed-rendering
// fix (F33 / plan stage B4): when the pane is shorter than the open tab
// list, View() must render a WINDOW that always contains the cursor, not
// unconditionally render every tab and rely on an outer MaxHeight clip to
// hide the overflow — a clip can silently chop off the cursor's own row.
func TestView_CursorStaysVisibleUnderOverflow(t *testing.T) {
	m := New(keymap.Default(), styles.Default())
	for i := int64(1); i <= 20; i++ {
		m = m.OpenFile(i, fmt.Sprintf("f%d.md", i))
	}
	// Header (1 row) + 5 tab rows visible — far fewer than the 20 open tabs.
	m = m.SetSize(30, 6)
	m = m.SetFocused(true)

	// Walk the cursor all the way to the bottom with repeated Down presses.
	for i := 0; i < 25; i++ {
		m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyDown})
	}
	if m.nav.Cursor != len(m.tabs)-1 {
		t.Fatalf("cursor = %d, want %d (last tab)", m.nav.Cursor, len(m.tabs)-1)
	}

	view := m.View()
	wantName := fmt.Sprintf("f%d.md", m.nav.Cursor+1)
	if !strings.Contains(view, wantName) {
		t.Fatalf("cursor tab %q not visible in overflowed view:\n%s", wantName, view)
	}

	// Walk back up to the top; the cursor's tab must stay visible the whole
	// way, not just at the extremes.
	for i := 0; i < 12; i++ {
		m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyUp})
	}
	view = m.View()
	wantName = fmt.Sprintf("f%d.md", m.nav.Cursor+1)
	if !strings.Contains(view, wantName) {
		t.Fatalf("cursor tab %q not visible after scrolling back up:\n%s", wantName, view)
	}
}
