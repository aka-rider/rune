package workspace

import (
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/help"
)

// openHelp is the F1 keypress (the help key).
func openHelp() tea.KeyPressMsg { return tea.KeyPressMsg{Code: tea.KeyF1} }

func TestHelpOpensReadOnlyTab(t *testing.T) {
	m := newTestWorkspace(t)
	m = loadFile(m, "a.md", "hello")
	m = focusEditor(m)

	m, _ = m.Update(openHelp())

	if !m.viewingHelp() {
		t.Fatal("expected to be viewing help after F1")
	}
	if m.filePath != help.DocPath {
		t.Fatalf("expected filePath %q, got %q", help.DocPath, m.filePath)
	}
	if !m.editor.ReadOnly() {
		t.Fatal("help document must be read-only")
	}
	if m.focus != paneCenter {
		t.Fatalf("expected focus paneCenter, got %v", m.focus)
	}
	if !strings.Contains(m.editor.Content(), "Rune Help") {
		t.Fatalf("editor should contain the help document, got %q", m.editor.Content())
	}
}

func TestHelpOpensFromAnyFocus(t *testing.T) {
	m := newTestWorkspace(t)
	m = loadFile(m, "a.md", "hello")
	m.focus = paneTree
	m, _ = m.Update(tea.WindowSizeMsg{Width: 80, Height: 24}) // flush focus

	m, _ = m.Update(openHelp())

	if !m.viewingHelp() {
		t.Fatal("F1 must open help even when the explorer is focused")
	}
	if m.focus != paneCenter {
		t.Fatalf("expected focus to move to the help doc (paneCenter), got %v", m.focus)
	}
}

func TestHelpToggleClosesAndRestoresEditable(t *testing.T) {
	// Start from the untitled startup state so closing help routes through
	// CreateUntitled (no disk read) rather than loadFileCmd.
	m := newTestWorkspace(t)
	m = focusEditor(m)

	m, _ = m.Update(openHelp()) // open
	if !m.viewingHelp() {
		t.Fatal("expected help open after first F1")
	}
	if !m.editor.ReadOnly() {
		t.Fatal("expected read-only while viewing help")
	}

	m, cmd := m.Update(openHelp()) // toggle off (focused) → close
	for _, msg := range execCmds(cmd) {
		m, _ = m.Update(msg)
	}

	if m.viewingHelp() {
		t.Fatal("expected help closed after second F1")
	}
	if m.editor.ReadOnly() {
		t.Fatal("editor must be editable after closing help")
	}
	if m.filePath != "" {
		t.Fatalf("expected untitled (empty filePath) after close, got %q", m.filePath)
	}
}

func TestSwitchFromHelpToFileClearsReadOnly(t *testing.T) {
	m := newTestWorkspace(t)
	m = focusEditor(m)

	m, _ = m.Update(openHelp())
	if !m.editor.ReadOnly() {
		t.Fatal("expected read-only help before switching")
	}

	m = loadFile(m, "a.md", "hello") // FileLoadedMsg clears read-only
	if m.editor.ReadOnly() {
		t.Fatal("editor must be editable after loading a real file")
	}
	if m.viewingHelp() {
		t.Fatal("should not be viewing help after loading a file")
	}
}

// TestTypingQuestionMarkDoesNotOpenHelp is the regression test for the original
// bug: a bare '?' must insert text, never toggle help.
func TestTypingQuestionMarkDoesNotOpenHelp(t *testing.T) {
	m := newTestWorkspace(t)
	m = loadFile(m, "a.md", "")
	m = focusEditor(m)

	m, _ = m.Update(tea.KeyPressMsg{Code: '?', Text: "?"})

	if m.viewingHelp() {
		t.Fatal("typing '?' must not open help")
	}
	if !strings.Contains(m.editor.Content(), "?") {
		t.Fatalf("typing '?' should insert it; content=%q", m.editor.Content())
	}
}

func TestHelpViewIsPure(t *testing.T) {
	m := newTestWorkspace(t)
	m = focusEditor(m)
	m, _ = m.Update(openHelp())

	v1 := m.View().Content
	v2 := m.View().Content
	if v1 != v2 {
		t.Fatal("View() must be pure while viewing help")
	}
}

// TestHelpScrollsWithArrowsAndPaging verifies the unified scroll fix: in the
// read-only help doc, Down scrolls one line and PageDown scrolls a page — the
// hidden cursor no longer pins the viewport.
func TestHelpScrollsWithArrowsAndPaging(t *testing.T) {
	m := newScrollWorkspace(t)
	m = focusEditor(m)
	m, _ = m.Update(openHelp())

	if off := m.editor.ScrollOffset(); off != 0 {
		t.Fatalf("help should start at top, got offset %d", off)
	}

	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyDown})
	if got := m.editor.ScrollOffset(); got != 1 {
		t.Fatalf("Down should scroll 1 line, got offset %d", got)
	}

	before := m.editor.ScrollOffset()
	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyPgDown})
	if got := m.editor.ScrollOffset(); got <= before+1 {
		t.Fatalf("PageDown should scroll a page, got offset %d (was %d)", got, before)
	}
}

// TestEditorPagingScrollsAPage verifies the paging fix applies to the normal
// editable editor too (not just read-only help).
func TestEditorPagingScrollsAPage(t *testing.T) {
	m := newScrollWorkspace(t)
	m = loadFile(m, "long.md", strings.Repeat("line\n", 200))
	m = focusEditor(m)

	if off := m.editor.ScrollOffset(); off != 0 {
		t.Fatalf("expected top, got %d", off)
	}
	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyPgDown})
	if got := m.editor.ScrollOffset(); got <= 1 {
		t.Fatalf("PageDown in the editor should scroll a page, got %d", got)
	}
}

// TestSwitchBackToUntitledRestoresContent verifies no data loss: typing in an
// untitled buffer, opening help, then Ctrl+1 back restores the typed content.
func TestSwitchBackToUntitledRestoresContent(t *testing.T) {
	m := newTestWorkspace(t) // starts with one "" untitled tab
	m = focusEditor(m)

	m, _ = m.Update(tea.KeyPressMsg{Code: 'h', Text: "h"})
	m, _ = m.Update(tea.KeyPressMsg{Code: 'i', Text: "i"})
	typed := m.editor.Content()
	if typed == "" {
		t.Fatal("expected typed content in the untitled buffer")
	}

	m, _ = m.Update(openHelp())
	if !m.viewingHelp() {
		t.Fatal("expected help open")
	}

	m, cmd := m.Update(tea.KeyPressMsg{Code: '1', Mod: tea.ModCtrl})
	for _, msg := range execCmds(cmd) {
		m, _ = m.Update(msg)
	}

	if m.viewingHelp() {
		t.Fatal("Ctrl+1 should switch away from help")
	}
	if m.filePath != "" {
		t.Fatalf("expected the untitled tab, got %q", m.filePath)
	}
	if m.editor.ReadOnly() {
		t.Fatal("untitled buffer must be editable")
	}
	if got := m.editor.Content(); got != typed {
		t.Fatalf("untitled content not restored: got %q, want %q", got, typed)
	}
}
