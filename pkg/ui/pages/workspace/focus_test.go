package workspace

import (
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/command"
	"rune/pkg/editor/keybind"
	"rune/pkg/terminal"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// focusedPanes lists the child panes reporting Focused()==true. The focus
// invariant is: after any Update, exactly one entry, matching m.focus.
func focusedPanes(m Model) []string {
	var f []string
	if m.title.Focused() {
		f = append(f, "title")
	}
	if m.filetree.Focused() {
		f = append(f, "filetree")
	}
	if m.opentabs.Focused() {
		f = append(f, "opentabs")
	}
	if m.editor.Focused() {
		f = append(f, "editor")
	}
	if m.chat.Focused() {
		f = append(f, "chat")
	}
	return f
}

// TestFocusProjectionInvariant: for every focus authority value, after an Update
// exactly one child reports focus and it matches the enum. Guards both the
// double-cursor (two focused) and dead-pane (zero/wrong focused) classes.
func TestFocusProjectionInvariant(t *testing.T) {
	cases := []struct {
		p    pane
		want string
	}{
		{paneTitle, "title"},
		{paneTree, "filetree"},
		{paneTabs, "opentabs"},
		{paneCenter, "editor"},
		{paneChat, "chat"},
	}
	for _, tc := range cases {
		m := newTestWorkspace(t)
		m.focus = tc.p
		// Any Update routes through finalize → applyFocus.
		m, _ = m.Update(tea.WindowSizeMsg{Width: 80, Height: 24})
		got := focusedPanes(m)
		if len(got) != 1 || got[0] != tc.want {
			t.Fatalf("focus=%v: focused panes=%v, want exactly [%s]", tc.p, got, tc.want)
		}
	}
}

// TestStartupFocusReachesFiletree: New() projects the initial paneTree focus so the
// explorer accepts keys at launch without waiting for a first non-key message.
func TestStartupFocusReachesFiletree(t *testing.T) {
	keys := keymap.Default()
	st := styles.Default()
	reg := command.NewBuilder().Build()
	res, _ := keybind.NewResolver(nil)
	m := New(keys, st, reg, res, terminal.TermCaps{}, "", nil)
	if m.focus != paneTree {
		t.Fatalf("expected initial focus paneTree, got %v", m.focus)
	}
	if !m.filetree.Focused() {
		t.Fatal("New() did not project initial focus onto the filetree")
	}
}

// TestFocusKeyFocusesPaneForNextKey: a keyboard-only focus change (^x) must make the
// target pane focused immediately, with no intervening non-key/broadcast message.
// Pre-fix the filetree's focused bool stayed false until a mouse click "revived" it.
func TestFocusKeyFocusesPaneForNextKey(t *testing.T) {
	m := newTestWorkspace(t)
	m.focus = paneCenter
	m, _ = m.Update(tea.WindowSizeMsg{Width: 80, Height: 24})
	if m.filetree.Focused() {
		t.Fatal("precondition: filetree should be unfocused while editing")
	}

	// ^x (FocusExplorer) is a KeyPressMsg — no broadcast path runs.
	m, _ = m.Update(tea.KeyPressMsg{Code: 'x', Mod: tea.ModCtrl})

	if m.focus != paneTree {
		t.Fatalf("^x did not set focus authority to paneTree, got %v", m.focus)
	}
	if !m.filetree.Focused() {
		t.Fatal("filetree not focused after ^x (dead-pane regression)")
	}
	if m.editor.Focused() {
		t.Fatal("editor still focused after ^x (double-focus regression)")
	}
}

// TestCreateNewFileShowsSingleCursor: ^n from a content-bearing editor focuses the
// title only — the editor must not also remain focused (the reported double cursor).
func TestCreateNewFileShowsSingleCursor(t *testing.T) {
	m := newTestWorkspace(t)
	m = loadFile(m, "a.md", "some content")
	m.focus = paneCenter
	m, _ = m.Update(tea.WindowSizeMsg{Width: 80, Height: 24})

	m, _ = m.Update(tea.KeyPressMsg{Code: 'n', Mod: tea.ModCtrl})

	if m.focus != paneTitle {
		t.Fatalf("^n did not focus the title, got %v", m.focus)
	}
	got := focusedPanes(m)
	if len(got) != 1 || got[0] != "title" {
		t.Fatalf("after ^n focused panes=%v, want exactly [title] (double-cursor regression)", got)
	}
}

// TestFocusSelfHealsOnNextUpdate: even if a child's focused bool is forced out of sync
// with the authority (the old failure mode), the very next Update reconciles it. This
// is what the mouse click did in the report; now any message does it.
func TestFocusSelfHealsOnNextUpdate(t *testing.T) {
	m := newTestWorkspace(t)
	m.focus = paneCenter
	m, _ = m.Update(tea.WindowSizeMsg{Width: 80, Height: 24})

	// Force a desync: authority is the editor, but the title bool is stuck on.
	m.title = m.title.SetFocused(true)
	if got := focusedPanes(m); len(got) != 2 {
		t.Fatalf("setup: expected desync (title+editor), got %v", got)
	}

	m, _ = m.Update(tea.WindowSizeMsg{Width: 80, Height: 24})

	got := focusedPanes(m)
	if len(got) != 1 || got[0] != "editor" {
		t.Fatalf("desync not healed: focused=%v, want [editor]", got)
	}
}
