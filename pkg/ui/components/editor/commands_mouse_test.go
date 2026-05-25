package editor

import (
	"fmt"
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/command"
	"rune/pkg/editor/keybind"
	"rune/pkg/terminal"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// newMouseTestEditor creates an editor ready for mouse tests with given content.
func newMouseTestEditor(content string) Model {
	keys := keymap.Default()
	st := styles.Default()
	reg := command.NewBuilder().Build()
	res, _ := keybind.NewResolver(nil)

	m := New(keys, st, reg, res, terminal.TermCaps{})
	m = m.SetFocused(true)
	m = m.SetSize(80, 24)
	m = m.SetContent("test.md", []byte(content))
	return m
}

// TestMouseClickPositionsCursor verifies that a click at a display position
// places the cursor at the correct buffer byte offset.
func TestMouseClickPositionsCursor(t *testing.T) {
	// "hello world\nline two\nthird"
	// Line 0: h(0) e(1) l(2) l(3) o(4) ' '(5) w(6) o(7) r(8) l(9) d(10) \n(11)
	// Line 1: l(12) i(13) n(14) e(15) ' '(16) t(17) w(18) o(19) \n(20)
	// Line 2: t(21) h(22) i(23) r(24) d(25)
	m := newMouseTestEditor("hello world\nline two\nthird")

	tests := []struct {
		name       string
		x, y       int
		wantOffset int
	}{
		{"start of line 0", 0, 1, 0},        // breadcrumb is row 0, so content starts at y=1
		{"middle of line 0", 5, 1, 5},       // col 5 -> offset 5 (space char)
		{"start of line 1", 0, 2, 12},       // line 1 starts at offset 12
		{"col 4 of line 1", 4, 2, 16},       // col 4 of line 1 -> offset 16 (space)
		{"start of line 2", 0, 3, 21},       // line 2 starts at offset 21
		{"beyond end of line 2", 10, 3, 26}, // clamp to buf.Len() (cursor-after-last-char)
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			msg := tea.MouseClickMsg{X: tt.x, Y: tt.y, Button: tea.MouseLeft}
			result, _ := m.handleMouseClick(msg, time.Now())
			primary := result.cursors.Primary()
			if primary.Position != tt.wantOffset {
				t.Errorf("click at (%d,%d): got offset %d, want %d",
					tt.x, tt.y, primary.Position, tt.wantOffset)
			}
			// Single click should collapse anchor == position (no selection)
			if primary.Anchor != primary.Position {
				t.Errorf("single click should not create selection: anchor=%d, position=%d",
					primary.Anchor, primary.Position)
			}
		})
	}
}

// TestMouseClickWithScrollOffset verifies click positioning accounts for scroll.
func TestMouseClickWithScrollOffset(t *testing.T) {
	content := "line0\nline1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9"
	m := newMouseTestEditor(content)

	// Scroll down 3 lines
	m.viewport.TopRow = 3

	// Click at display row 0 (first visible row after breadcrumb) should map to buffer line 3
	msg := tea.MouseClickMsg{X: 0, Y: 1, Button: tea.MouseLeft}
	result, _ := m.handleMouseClick(msg, time.Now())
	primary := result.cursors.Primary()

	// line3 starts at offset: "line0\nline1\nline2\n" = 6+6+6 = 18
	wantOffset := 18
	if primary.Position != wantOffset {
		t.Errorf("click with scroll offset 3: got offset %d, want %d",
			primary.Position, wantOffset)
	}
}

// TestMouseDoubleClickSelectsWord verifies double-click word selection.
func TestMouseDoubleClickSelectsWord(t *testing.T) {
	m := newMouseTestEditor("hello world")

	now := time.Now()

	// First click at col 1 (inside "hello") - breadcrumb is row 0
	msg := tea.MouseClickMsg{X: 1, Y: 1, Button: tea.MouseLeft}
	m, _ = m.handleMouseClick(msg, now)

	// Second click at same position within threshold -> double-click
	msg2 := tea.MouseClickMsg{X: 1, Y: 1, Button: tea.MouseLeft}
	result, _ := m.handleMouseClick(msg2, now.Add(100*time.Millisecond))
	primary := result.cursors.Primary()

	// "hello" is bytes 0-5, anchor at 0 (word start), position at 5 (word end)
	if primary.Anchor != 0 {
		t.Errorf("double-click word select: anchor=%d, want 0", primary.Anchor)
	}
	if primary.Position != 5 {
		t.Errorf("double-click word select: position=%d, want 5", primary.Position)
	}
}

// TestMouseTripleClickSelectsLine verifies triple-click selects the entire line.
func TestMouseTripleClickSelectsLine(t *testing.T) {
	m := newMouseTestEditor("hello world\nline two\nthird")

	now := time.Now()

	// Triple click on line 0 at col 3
	msg := tea.MouseClickMsg{X: 3, Y: 1, Button: tea.MouseLeft}
	m, _ = m.handleMouseClick(msg, now)
	m, _ = m.handleMouseClick(msg, now.Add(100*time.Millisecond))
	result, _ := m.handleMouseClick(msg, now.Add(200*time.Millisecond))
	primary := result.cursors.Primary()

	// Line 0 spans bytes 0-12 (including the \n, since line 1 starts at 12)
	if primary.Anchor != 0 {
		t.Errorf("triple-click line select: anchor=%d, want 0", primary.Anchor)
	}
	if primary.Position != 12 {
		t.Errorf("triple-click line select: position=%d, want 12", primary.Position)
	}
}

// TestMouseShiftClickExtendsSelection verifies shift+click extends from anchor.
func TestMouseShiftClickExtendsSelection(t *testing.T) {
	m := newMouseTestEditor("hello world\nline two")

	now := time.Now()

	// First click at col 2 of line 0 -> positions cursor at offset 2
	msg := tea.MouseClickMsg{X: 2, Y: 1, Button: tea.MouseLeft}
	m, _ = m.handleMouseClick(msg, now)

	// Shift+click at col 8 of line 0 -> extends selection
	shiftMsg := tea.MouseClickMsg{X: 8, Y: 1, Button: tea.MouseLeft, Mod: tea.ModShift}
	result, _ := m.handleMouseClick(shiftMsg, now.Add(time.Second))
	primary := result.cursors.Primary()

	// Anchor should stay at 2 (original position), position moves to 8
	if primary.Anchor != 2 {
		t.Errorf("shift+click: anchor=%d, want 2", primary.Anchor)
	}
	if primary.Position != 8 {
		t.Errorf("shift+click: position=%d, want 8", primary.Position)
	}
}

// TestMouseAltClickAddsCursor verifies alt+click adds a secondary cursor.
func TestMouseAltClickAddsCursor(t *testing.T) {
	m := newMouseTestEditor("hello world\nline two")

	now := time.Now()

	// First click at col 0 -> single cursor at offset 0
	msg := tea.MouseClickMsg{X: 0, Y: 1, Button: tea.MouseLeft}
	m, _ = m.handleMouseClick(msg, now)

	if m.cursors.Len() != 1 {
		t.Fatalf("expected 1 cursor after first click, got %d", m.cursors.Len())
	}

	// Alt+click at col 0 of line 1 -> add cursor at offset 12
	altMsg := tea.MouseClickMsg{X: 0, Y: 2, Button: tea.MouseLeft, Mod: tea.ModAlt}
	result, _ := m.handleMouseClick(altMsg, now.Add(time.Second))

	if result.cursors.Len() != 2 {
		t.Errorf("alt+click: expected 2 cursors, got %d", result.cursors.Len())
	}
}

// TestMouseScrollDoesNotMoveCursor verifies scroll wheel doesn't change cursor position.
func TestMouseScrollDoesNotMoveCursor(t *testing.T) {
	// Need enough lines to exceed viewport height (24 - 1 breadcrumb = 23 content lines)
	var lines []string
	for i := 0; i < 40; i++ {
		lines = append(lines, fmt.Sprintf("line%d", i))
	}
	content := strings.Join(lines, "\n")
	m := newMouseTestEditor(content)

	now := time.Now()

	// Position cursor at offset 3 (inside "line0")
	clickMsg := tea.MouseClickMsg{X: 3, Y: 1, Button: tea.MouseLeft}
	m, _ = m.handleMouseClick(clickMsg, now)
	cursorBefore := m.cursors.Primary().Position

	// Scroll down
	scrollMsg := tea.MouseWheelMsg{Button: tea.MouseWheelDown}
	result, _ := m.handleMouseWheel(scrollMsg)

	cursorAfter := result.cursors.Primary().Position
	if cursorAfter != cursorBefore {
		t.Errorf("scroll moved cursor: before=%d, after=%d", cursorBefore, cursorAfter)
	}

	// Verify viewport actually scrolled
	if result.viewport.TopRow <= 0 {
		t.Errorf("scroll didn't move viewport: TopRow=%d", result.viewport.TopRow)
	}
}

// TestMouseScrollUp verifies scroll up behavior.
func TestMouseScrollUp(t *testing.T) {
	content := "line0\nline1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9"
	m := newMouseTestEditor(content)
	m.viewport.TopRow = 5

	scrollMsg := tea.MouseWheelMsg{Button: tea.MouseWheelUp}
	result, _ := m.handleMouseWheel(scrollMsg)

	expected := 5 - mouseScrollLines
	if result.viewport.TopRow != expected {
		t.Errorf("scroll up: TopRow=%d, want %d", result.viewport.TopRow, expected)
	}
}

// TestMouseScrollUpClampsToZero verifies scroll up doesn't go below 0.
func TestMouseScrollUpClampsToZero(t *testing.T) {
	m := newMouseTestEditor("hello\nworld")
	m.viewport.TopRow = 1

	scrollMsg := tea.MouseWheelMsg{Button: tea.MouseWheelUp}
	result, _ := m.handleMouseWheel(scrollMsg)

	if result.viewport.TopRow < 0 {
		t.Errorf("scroll up went negative: TopRow=%d", result.viewport.TopRow)
	}
}

// TestMouseDragCreatesSelection verifies click+drag creates a selection.
func TestMouseDragCreatesSelection(t *testing.T) {
	m := newMouseTestEditor("hello world")

	now := time.Now()

	// Click at col 2 -> sets anchor at offset 2
	clickMsg := tea.MouseClickMsg{X: 2, Y: 1, Button: tea.MouseLeft}
	m, _ = m.handleMouseClick(clickMsg, now)

	// Drag (motion) to col 7
	motionMsg := tea.MouseMotionMsg{X: 7, Y: 1, Button: tea.MouseLeft}
	result, _ := m.handleMouseMotion(motionMsg)
	primary := result.cursors.Primary()

	// Anchor at 2, position at 7
	if primary.Anchor != 2 {
		t.Errorf("drag: anchor=%d, want 2", primary.Anchor)
	}
	if primary.Position != 7 {
		t.Errorf("drag: position=%d, want 7", primary.Position)
	}
	if !primary.HasSelection() {
		t.Error("drag should create selection")
	}
}

// TestMouseClickUnfocusedIgnored verifies mouse events are ignored when unfocused.
func TestMouseClickUnfocusedIgnored(t *testing.T) {
	m := newMouseTestEditor("hello world")
	m = m.SetFocused(false)

	msg := tea.MouseClickMsg{X: 5, Y: 1, Button: tea.MouseLeft}
	result, _ := m.handleMouseClick(msg, time.Now())

	// Cursor should not have moved (still default empty)
	if result.cursors.Len() > 0 {
		primary := result.cursors.Primary()
		if primary.Position != 0 {
			t.Errorf("unfocused click moved cursor to %d", primary.Position)
		}
	}
}

// TestMouseRightClickIgnored verifies only left button is handled.
func TestMouseRightClickIgnored(t *testing.T) {
	m := newMouseTestEditor("hello world")

	// Set initial position
	now := time.Now()
	clickMsg := tea.MouseClickMsg{X: 3, Y: 1, Button: tea.MouseLeft}
	m, _ = m.handleMouseClick(clickMsg, now)
	posBefore := m.cursors.Primary().Position

	// Right click at different position
	rightMsg := tea.MouseClickMsg{X: 8, Y: 1, Button: tea.MouseRight}
	result, _ := m.handleMouseClick(rightMsg, now.Add(time.Second))
	posAfter := result.cursors.Primary().Position

	if posAfter != posBefore {
		t.Errorf("right click moved cursor: before=%d, after=%d", posBefore, posAfter)
	}
}

// TestMouseClickAboveBreadcrumbIgnored verifies clicks in breadcrumb area are not handled.
func TestMouseClickAboveBreadcrumbIgnored(t *testing.T) {
	m := newMouseTestEditor("hello world")

	// Click at Y=0 (breadcrumb row) should be ignored
	msg := tea.MouseClickMsg{X: 3, Y: 0, Button: tea.MouseLeft}
	result, _ := m.handleMouseClick(msg, time.Now())

	// Should not have positioned cursor
	if result.cursors.Len() > 0 && result.cursors.Primary().Position != 0 {
		t.Errorf("click on breadcrumb moved cursor")
	}
}

// TestMouseDoubleClickSelectsWordWithPunctuation verifies word boundaries with mixed chars.
func TestMouseDoubleClickSelectsWordWithPunctuation(t *testing.T) {
	m := newMouseTestEditor("foo.bar baz")

	now := time.Now()

	// Double-click on 'b' at col 4 (inside "bar")
	// "foo.bar baz"
	// col:  0123456789...
	// 'b' is at offset 4
	msg := tea.MouseClickMsg{X: 4, Y: 1, Button: tea.MouseLeft}
	m, _ = m.handleMouseClick(msg, now)
	result, _ := m.handleMouseClick(msg, now.Add(100*time.Millisecond))
	primary := result.cursors.Primary()

	// "bar" spans offsets 4-7 (b=4, a=5, r=6, space at 7)
	if primary.Anchor != 4 {
		t.Errorf("double-click punctuation: anchor=%d, want 4", primary.Anchor)
	}
	if primary.Position != 7 {
		t.Errorf("double-click punctuation: position=%d, want 7", primary.Position)
	}
}

// TestMouseMultiClickResetAfterThreshold verifies click count resets after timeout.
func TestMouseMultiClickResetAfterThreshold(t *testing.T) {
	m := newMouseTestEditor("hello world")

	now := time.Now()

	// First click
	msg := tea.MouseClickMsg{X: 1, Y: 1, Button: tea.MouseLeft}
	m, _ = m.handleMouseClick(msg, now)

	// Second click after threshold -> should be treated as a new single click
	m, _ = m.handleMouseClick(msg, now.Add(600*time.Millisecond))

	if m.mouse.clickCount != 1 {
		t.Errorf("click after threshold: clickCount=%d, want 1", m.mouse.clickCount)
	}
}

// TestMouseTripleClickLastLine verifies triple-click on the last line (no trailing newline).
func TestMouseTripleClickLastLine(t *testing.T) {
	m := newMouseTestEditor("first\nsecond")

	now := time.Now()

	// Triple click on line 1 (the last line)
	msg := tea.MouseClickMsg{X: 2, Y: 2, Button: tea.MouseLeft}
	m, _ = m.handleMouseClick(msg, now)
	m, _ = m.handleMouseClick(msg, now.Add(100*time.Millisecond))
	result, _ := m.handleMouseClick(msg, now.Add(200*time.Millisecond))
	primary := result.cursors.Primary()

	// Line 1 starts at offset 6 ("second" = bytes 6..12)
	// Last line: lineEnd should be buf.Len() = 12
	if primary.Anchor != 6 {
		t.Errorf("triple-click last line: anchor=%d, want 6", primary.Anchor)
	}
	if primary.Position != 12 {
		t.Errorf("triple-click last line: position=%d, want 12", primary.Position)
	}
}
