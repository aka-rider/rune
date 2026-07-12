package workspace

import (
	"errors"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"
)

// ─────────────────────────────────────────────────────────────────────────────
// Gate 4: Global keys do NOT produce text insertion in editor
// ─────────────────────────────────────────────────────────────────────────────

func TestGate4_GlobalKeysDoNotInsertText(t *testing.T) {
	m := newTestWorkspace(t)
	m = loadFile(m, "a.txt", "hello")
	m.focus = paneCenter

	original := m.editor.Content()

	// Send focus explorer key (ctrl+x).
	m, _ = m.Update(tea.KeyPressMsg{Code: 'x', Mod: tea.ModCtrl})
	if m.editor.Content() != original {
		t.Fatal("ctrl+x (FocusExplorer) inserted text in editor")
	}

	// Send zen mode key (ctrl+o).
	m.focus = paneCenter
	m, _ = m.Update(tea.KeyPressMsg{Code: 'o', Mod: tea.ModCtrl})
	if m.editor.Content() != original {
		t.Fatal("ctrl+o (ZenMode) inserted text in editor")
	}

	// Send focus editor key (ctrl+e).
	m.focus = paneCenter
	m, _ = m.Update(tea.KeyPressMsg{Code: 'e', Mod: tea.ModCtrl})
	if m.editor.Content() != original {
		t.Fatal("ctrl+e (FocusEditor) inserted text in editor")
	}
}

// D9: the former "Gate 11: editor.WantsModalInput() routes keys to editor
// before globals" test lived here — its body asserted nothing (an `if` with
// no `t.Fatal` on either branch, so it passed unconditionally) and was named
// for a `WantsModalInput` symbol that no longer exists anywhere in this
// codebase. Deleted rather than reconstructed: TestGate4 above already
// covers "a global key (ctrl+o zen toggle) is not swallowed as text
// insertion", the real behavior this test's own body actually exercised.

// ─────────────────────────────────────────────────────────────────────────────
// Same-file switch is a no-op (no load issued)
// ─────────────────────────────────────────────────────────────────────────────

func TestSameFileOpenIsNoop(t *testing.T) {
	m := newTestWorkspace(t)
	m = loadFile(m, "a.txt", "content A")

	// Open same file — should be a no-op (no cmd issued).
	_, cmd := m.requestOpenPath(0, "a.txt")
	if cmd != nil {
		t.Fatal("opening same file should not issue a load cmd")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Bug fix: ^W on last tab resets editor to Untitled
// ─────────────────────────────────────────────────────────────────────────────

func TestCloseLastTabResetsToUntitled(t *testing.T) {
	m := newTestWorkspace(t)
	m = loadFile(m, "a.txt", "content A")

	// Only one tab — closing it must reset the editor to a fresh untitled buffer.
	m, cmd := m.requestCloseCurrent()
	msgs := execCmds(cmd)
	for _, msg := range msgs {
		m, cmd = m.Update(msg)
		msgs = append(msgs, execCmds(cmd)...)
	}

	// Title should start with "Untitled" (owned by workspace title component — D6).
	titleText := m.title.Text()
	if !strings.HasPrefix(titleText, "Untitled") {
		t.Fatalf("expected title starting with 'Untitled', got %q", titleText)
	}
	// Workspace must have no file path.
	if m.view.Path() != "" {
		t.Fatalf("expected empty file path after last-tab close, got %q", m.view.Path())
	}
	// Opentabs should show exactly one tab (the new untitled slot) with empty path.
	if path := m.opentabs.PathAt(0); path != "" {
		t.Fatalf("expected untitled tab with empty path, got %q", path)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// A bind-new conflict (file already exists) keeps the buffer untitled and is
// NOT silently bound — guards against the optimistic-bind clobber (rung 1).
// ─────────────────────────────────────────────────────────────────────────────

func TestBindNewConflict_KeepsBufferUntitled(t *testing.T) {
	m := newTestWorkspace(t)
	// Simulate an in-flight bind-new (naming an untitled).
	m.activeSave = SaveIdentity{RequestID: "bind-1", InFlight: true}
	m, _ = m.Update(FileSaveErrorMsg{
		Path:      "foo.md",
		RequestID: "bind-1",
		Conflict:  true,
		Err:       errors.New(`materialize "foo.md": file already exists`),
	})
	if m.view.Path() != "" {
		t.Fatalf("expected buffer to stay untitled after bind conflict, got filePath %q", m.view.Path())
	}
	if m.activeSave.InFlight {
		t.Fatal("expected activeSave cleared after bind conflict")
	}
	if m.guard.close.active || m.guard.evict.active || m.guard.quit.active {
		t.Fatal("expected pending data-loss action cleared after a failed guard save")
	}
}
