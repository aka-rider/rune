package editor

import (
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
)

// clickModelLine synthesizes a left mouse click at the given model line and
// line-relative display column, accounting for the editor's header and offset.
// It returns the Cmd produced by the click.
func clickModelLine(t *testing.T, m Model, modelLine, dispCol int) (Model, tea.Cmd) {
	t.Helper()
	y := modelLine + m.offsetY + m.headerHeight()
	x := dispCol + m.offsetX
	return m.handleMouseClick(tea.MouseClickMsg{Button: tea.MouseLeft, X: x, Y: y}, time.Now())
}

// TestLinkClick_WikiLinkAfterFirstLine reproduces BUG3: clicking a wiki link
// that sits on a line AFTER line 0 must navigate (emit LinkClickedMsg with a
// resolved path), not silently fail because of a line-relative vs document-wide
// column mismatch.
func TestLinkClick_WikiLinkAfterFirstLine(t *testing.T) {
	m := newTestEditor("")
	m = m.SetContent("/tmp/notes/doc.md", []byte("line0\nline1\n[[note]]\nafter"))
	m = m.SetSize(80, 24)
	m = m.SetFocused(true)

	// Click within the folded "note" label on model line 2 (column 1).
	m, cmd := clickModelLine(t, m, 2, 1)
	msgs := runCmd(t, cmd)
	clicked, ok := firstMsg[LinkClickedMsg](msgs)
	if !ok {
		t.Fatalf("expected LinkClickedMsg from clicking a wiki link on line 2, got %+v", msgs)
	}
	if clicked.Path == "" {
		t.Error("LinkClickedMsg.Path is empty; wiki link should resolve to a target")
	}
}

// TestLinkClick_MarkdownLinkAfterFirstLine verifies the same fix for a markdown
// link with a local (resolvable) target on a non-first line.
func TestLinkClick_MarkdownLinkAfterFirstLine(t *testing.T) {
	m := newTestEditor("")
	m = m.SetContent("/tmp/notes/doc.md", []byte("line0\n[text](other.md)\nafter"))
	m = m.SetSize(80, 24)
	m = m.SetFocused(true)

	// Click within the folded "text" label on model line 1 (column 1).
	m, cmd := clickModelLine(t, m, 1, 1)
	msgs := runCmd(t, cmd)
	clicked, ok := firstMsg[LinkClickedMsg](msgs)
	if !ok {
		t.Fatalf("expected LinkClickedMsg from clicking a markdown link on line 1, got %+v", msgs)
	}
	if clicked.Path == "" {
		t.Error("LinkClickedMsg.Path is empty; local markdown link should resolve")
	}
}
