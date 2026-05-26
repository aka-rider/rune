package editor

import (
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/keybind"
	"rune/pkg/terminal"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

func newSelectionTestEditor(content string) Model {
	keys := keymap.Default()
	st := styles.Default()
	reg := command.NewBuilder().Build()
	resolver, _ := keybind.NewResolver(nil)

	m := New(keys, st, reg, resolver, terminal.TermCaps{})
	m = m.SetSize(80, 24)
	m = m.SetFocused(true)
	m.buf = buffer.New(content)
	m.cursors = cursor.NewCursorSet(0)
	m = m.syncDisplay()
	return m
}

// TestSpec_SelectionRendering_VisibleHighlight verifies that selected text is
// rendered with a distinct style (ANSI codes) compared to unselected text.
func TestSpec_SelectionRendering_VisibleHighlight(t *testing.T) {
	m := newSelectionTestEditor("hello world")

	// No selection: capture baseline view
	baseView := m.View()

	// Set a selection: "hello" is selected (anchor=0, position=5)
	m.cursors = cursor.NewCursorSetFrom([]cursor.Cursor{
		{Position: 5, Anchor: 0, ID: 1},
	})
	m = m.syncDisplay()
	selView := m.View()

	if baseView == selView {
		t.Error("View() with selection is identical to View() without selection — selection not rendered")
	}

	// The selection view should have ANSI escape sequences for the selection style
	if !strings.Contains(selView, "\x1b[") {
		t.Error("View() with selection contains no ANSI sequences — selection style not applied")
	}
}

// TestSpec_SelectionRendering_CursorOnTopOfSelection verifies that the cursor
// position (reverse-video) is visually distinct even within a selection.
func TestSpec_SelectionRendering_CursorOnTopOfSelection(t *testing.T) {
	m := newSelectionTestEditor("hello world")

	// Selection from 0 to 5 with Position at 5 (cursor is at the selection edge)
	m.cursors = cursor.NewCursorSetFrom([]cursor.Cursor{
		{Position: 5, Anchor: 0, ID: 1},
	})
	m = m.syncDisplay()
	view := m.View()

	// Count distinct ANSI sequences — should have both selection BG and cursor reverse
	// The "\x1b[7m" is reverse video (cursor); selection uses background color.
	if !strings.Contains(view, "\x1b[") {
		t.Error("no styling in view with selection + cursor")
	}

	// Position=5 is the space character ' '. The cursor should still be at that position.
	// Verify by checking the reverse sequence appears (cursor rendering present).
	if !strings.Contains(view, "\x1b[7m") {
		t.Error("cursor reverse-video not found in selection view")
	}
}

// TestSpec_SelectionRendering_BackwardSelection verifies that a backward
// selection (Position < Anchor) is rendered the same as a forward one.
func TestSpec_SelectionRendering_BackwardSelection(t *testing.T) {
	m := newSelectionTestEditor("hello world")

	// Forward selection: anchor=0, position=5
	m.cursors = cursor.NewCursorSetFrom([]cursor.Cursor{
		{Position: 5, Anchor: 0, ID: 1},
	})
	m = m.syncDisplay()
	fwdView := m.View()

	// Backward selection: anchor=5, position=0
	m.cursors = cursor.NewCursorSetFrom([]cursor.Cursor{
		{Position: 0, Anchor: 5, ID: 1},
	})
	m = m.syncDisplay()
	bwdView := m.View()

	// Both should have selection styling (views differ because cursor position differs,
	// but both must contain selection-style ANSI codes)
	if !strings.Contains(fwdView, "\x1b[") {
		t.Error("forward selection has no ANSI styling")
	}
	if !strings.Contains(bwdView, "\x1b[") {
		t.Error("backward selection has no ANSI styling")
	}

	// Both should differ from a no-selection view
	m.cursors = cursor.NewCursorSet(0)
	m = m.syncDisplay()
	noSelView := m.View()

	if fwdView == noSelView {
		t.Error("forward selection view identical to no-selection view")
	}
	if bwdView == noSelView {
		t.Error("backward selection view identical to no-selection view")
	}
}

// TestSpec_SelectionRendering_MultiCursor verifies that multiple cursors with
// independent selections are all visually highlighted.
func TestSpec_SelectionRendering_MultiCursor(t *testing.T) {
	m := newSelectionTestEditor("hello world foobar")

	// Two cursors, each with a selection
	m.cursors = cursor.NewCursorSetFrom([]cursor.Cursor{
		{Position: 5, Anchor: 0, ID: 1},   // "hello" selected
		{Position: 17, Anchor: 12, ID: 2}, // "fooba" selected
	})
	m = m.syncDisplay()
	view := m.View()

	// Single cursor with no selection
	m.cursors = cursor.NewCursorSet(0)
	m = m.syncDisplay()
	noSelView := m.View()

	if view == noSelView {
		t.Error("multi-cursor selection view identical to no-selection view")
	}
}

// TestSpec_SelectionRendering_Unfocused verifies that an unfocused editor does
// NOT render selection highlighting.
func TestSpec_SelectionRendering_Unfocused(t *testing.T) {
	m := newSelectionTestEditor("hello world")

	m.cursors = cursor.NewCursorSetFrom([]cursor.Cursor{
		{Position: 5, Anchor: 0, ID: 1},
	})
	m = m.syncDisplay()
	focusedView := m.View()

	m = m.SetFocused(false)
	unfocusedView := m.View()

	if focusedView == unfocusedView {
		t.Error("focused and unfocused views with selection are identical")
	}
}

// TestIntegration_ShiftArrowSelectionVisible is the KEY missing gate test:
// it feeds Shift+Right key events and asserts the resulting View() contains
// selection styling.
func TestIntegration_ShiftArrowSelectionVisible(t *testing.T) {
	keys := keymap.Default()
	st := styles.Default()
	builder := command.NewBuilder()
	builder, _ = RegisterCommands(builder)
	reg := builder.Build()
	bindings, err := keys.CommandBindings()
	if err != nil {
		t.Fatalf("CommandBindings: %v", err)
	}
	resolver, err := keybind.NewResolver(bindings)
	if err != nil {
		t.Fatalf("NewResolver: %v", err)
	}

	m := New(keys, st, reg, resolver, terminal.TermCaps{})
	m = m.SetSize(80, 24)
	m = m.SetFocused(true)
	m = m.SetContent("test.txt", []byte("hello world"))

	// View before any selection
	baseView := m.View()

	// Feed Shift+Right 5 times to select "hello"
	for i := 0; i < 5; i++ {
		m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyRight, Mod: tea.ModShift})
	}

	// Verify model state: selection should exist
	primary := m.cursors.Primary()
	if !primary.HasSelection() {
		t.Fatal("expected selection after Shift+Right, but HasSelection() is false")
	}
	if primary.Position != 5 {
		t.Errorf("expected cursor position 5, got %d", primary.Position)
	}
	if primary.Anchor != 0 {
		t.Errorf("expected anchor 0, got %d", primary.Anchor)
	}

	// KEY ASSERTION: View() must visually differ from the no-selection state
	selView := m.View()
	if selView == baseView {
		t.Error("View() after Shift+Right is identical to View() without selection — " +
			"selection highlighting is not rendered")
	}
}

// TestSpec_SelectionRendering_SpanBoundary verifies selection renders correctly
// when it spans across multiple display spans (e.g., plain text and a word boundary).
func TestSpec_SelectionRendering_SpanBoundary(t *testing.T) {
	// Use content that will produce multiple spans in revealed mode
	m := newSelectionTestEditor("abc def ghi")

	// Select across what might be multiple spans in the display
	m.cursors = cursor.NewCursorSetFrom([]cursor.Cursor{
		{Position: 8, Anchor: 2, ID: 1}, // "c def g" selected
	})
	m = m.syncDisplay()
	view := m.View()

	// No-selection baseline
	m.cursors = cursor.NewCursorSet(0)
	m = m.syncDisplay()
	noSelView := m.View()

	if view == noSelView {
		t.Error("selection spanning multiple positions not rendered differently")
	}
}
