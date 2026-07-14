package main

import (
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/styles"
	"rune/pkg/workspaceroot"
)

func testPrompt() *workspaceroot.Prompt {
	return &workspaceroot.Prompt{
		Candidates: []workspaceroot.Candidate{
			{Dir: "/home/alice/repo", Kind: workspaceroot.KindProject},
			{Dir: "/home/alice/repo/src", Kind: workspaceroot.KindHere},
			{Dir: "/home/alice", Kind: workspaceroot.KindGlobal},
			{Dir: "/home/alice/repo/src", Kind: workspaceroot.KindMemory},
		},
		Default: 0,
	}
}

// --- §8.1 Render Purity: View() N times -> identical output, no side effects ---

func TestRootChooser_RenderPurity(t *testing.T) {
	m := newRootChooser(testPrompt(), styles.Default())
	updated, _ := m.Update(tea.WindowSizeMsg{Width: 100, Height: 40})
	m = updated.(rootChooser)

	first := m.render()
	for i := range 5 {
		got := m.render()
		if got != first {
			t.Fatalf("View() not pure: render %d differs from first\nfirst:\n%s\ngot:\n%s", i, first, got)
		}
	}
	// Model state must be unchanged by repeated renders.
	if m.cursor != 0 {
		t.Fatalf("cursor mutated by View(): %d", m.cursor)
	}
}

func TestRootChooser_ViewNonEmpty(t *testing.T) {
	m := newRootChooser(testPrompt(), styles.Default())
	view := m.View()
	if view.Content == "" {
		t.Fatal("expected non-empty view content")
	}
}

// --- §8.1 Key Routing: arrows move, Enter emits selected dir, Esc -> quit ---

func TestRootChooser_ArrowDownMovesCursor(t *testing.T) {
	m := newRootChooser(testPrompt(), styles.Default())
	if m.cursor != 0 {
		t.Fatalf("expected initial cursor at Default (0), got %d", m.cursor)
	}

	var updated tea.Model
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyDown})
	m = updated.(rootChooser)
	if m.cursor != 1 {
		t.Fatalf("cursor after KeyDown = %d, want 1", m.cursor)
	}

	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyDown})
	m = updated.(rootChooser)
	if m.cursor != 2 {
		t.Fatalf("cursor after 2nd KeyDown = %d, want 2", m.cursor)
	}

	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyDown})
	m = updated.(rootChooser)
	if m.cursor != 3 {
		t.Fatalf("cursor after 3rd KeyDown = %d, want 3", m.cursor)
	}

	// Clamped at the last candidate — no wraparound.
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyDown})
	m = updated.(rootChooser)
	if m.cursor != 3 {
		t.Fatalf("cursor overshot past last candidate: %d", m.cursor)
	}
}

func TestRootChooser_ArrowUpMovesCursor(t *testing.T) {
	prompt := testPrompt()
	prompt.Default = 2
	m := newRootChooser(prompt, styles.Default())

	var updated tea.Model
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyUp})
	m = updated.(rootChooser)
	if m.cursor != 1 {
		t.Fatalf("cursor after KeyUp = %d, want 1", m.cursor)
	}

	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyUp})
	m = updated.(rootChooser)
	updated, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyUp})
	m = updated.(rootChooser)
	if m.cursor != 0 {
		t.Fatalf("cursor undershot below first candidate: %d", m.cursor)
	}
}

func TestRootChooser_EnterEmitsSelectedDir(t *testing.T) {
	prompt := testPrompt()
	prompt.Default = 1 // "/home/alice/repo/src"
	m := newRootChooser(prompt, styles.Default())

	updated, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(rootChooser)

	candidate, ok := m.Chosen()
	if !ok {
		t.Fatal("expected Chosen ok=true after Enter")
	}
	if candidate.Dir != "/home/alice/repo/src" {
		t.Fatalf("Chosen dir = %q, want /home/alice/repo/src", candidate.Dir)
	}
	if candidate.Kind != workspaceroot.KindHere {
		t.Fatalf("Chosen kind = %v, want KindHere", candidate.Kind)
	}
	if cmd == nil {
		t.Fatal("expected a quit Cmd after Enter")
	}
}

func TestRootChooser_EnterOnMemoryCandidateReturnsKindMemory(t *testing.T) {
	prompt := testPrompt()
	prompt.Default = 3 // the trailing KindMemory candidate
	m := newRootChooser(prompt, styles.Default())

	updated, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	m = updated.(rootChooser)

	candidate, ok := m.Chosen()
	if !ok {
		t.Fatal("expected Chosen ok=true after Enter")
	}
	if candidate.Kind != workspaceroot.KindMemory {
		t.Fatalf("Chosen kind = %v, want KindMemory", candidate.Kind)
	}
	if cmd == nil {
		t.Fatal("expected a quit Cmd after Enter")
	}
}

func TestRootChooser_RenderShowsNoneForMemoryCandidate(t *testing.T) {
	prompt := testPrompt()
	prompt.Default = 3 // the trailing KindMemory candidate
	m := newRootChooser(prompt, styles.Default())
	updated, _ := m.Update(tea.WindowSizeMsg{Width: 100, Height: 40})
	m = updated.(rootChooser)

	rendered := m.render()
	if !strings.Contains(rendered, "None") {
		t.Fatalf("expected rendered output to contain %q, got:\n%s", "None", rendered)
	}
	if strings.Contains(rendered, "/.rune (memory)") {
		t.Fatalf("expected rendered output to NOT show a fake disk path for the memory candidate, got:\n%s", rendered)
	}
}

func TestRootChooser_EscQuits(t *testing.T) {
	m := newRootChooser(testPrompt(), styles.Default())

	updated, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyEscape})
	m = updated.(rootChooser)

	_, ok := m.Chosen()
	if ok {
		t.Fatal("expected Chosen ok=false after Esc")
	}
	if !m.quit {
		t.Fatal("expected quit=true after Esc")
	}
	if cmd == nil {
		t.Fatal("expected a quit Cmd after Esc")
	}
}

func TestRootChooser_CtrlCQuits(t *testing.T) {
	m := newRootChooser(testPrompt(), styles.Default())

	updated, cmd := m.Update(tea.KeyPressMsg{Code: 'c', Mod: tea.ModCtrl})
	m = updated.(rootChooser)

	if !m.quit {
		t.Fatal("expected quit=true after Ctrl+C")
	}
	if cmd == nil {
		t.Fatal("expected a quit Cmd after Ctrl+C")
	}
}

func TestRootChooser_UnboundKeyIgnored(t *testing.T) {
	m := newRootChooser(testPrompt(), styles.Default())

	updated, cmd := m.Update(tea.KeyPressMsg{Text: "x"})
	m2 := updated.(rootChooser)
	if m2.cursor != m.cursor || m2.quit || m2.decided {
		t.Fatalf("expected no state change on an unbound key, got %+v", m2)
	}
	if cmd != nil {
		t.Fatal("expected no Cmd for an unbound key")
	}
}

func TestRootChooser_DefaultCursorClampedIfOutOfRange(t *testing.T) {
	prompt := testPrompt()
	prompt.Default = 99 // out of range — defensive clamp
	m := newRootChooser(prompt, styles.Default())
	if m.cursor != 0 {
		t.Fatalf("expected clamped cursor 0, got %d", m.cursor)
	}
}
