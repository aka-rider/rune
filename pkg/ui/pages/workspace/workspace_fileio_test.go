package workspace

import (
	"os"
	"path/filepath"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/docstate"
	"rune/pkg/vfs"
)

// tabHasDocID reports whether any open tab carries the given VFS doc id.
func tabHasDocID(m Model, id int64) bool {
	for i := 0; i < m.opentabs.Len(); i++ {
		if m.opentabs.DocIDAt(i) == id {
			return true
		}
	}
	return false
}

// TestMaterialize_BindNewRefusesClobber: naming an untitled over an existing
// file must NOT overwrite it (Catastrophic, rung 1 — CLAUDE.md §1.4.1).
func TestMaterialize_BindNewRefusesClobber(t *testing.T) {
	path := filepath.Join(t.TempDir(), "exists.md")
	if err := os.WriteFile(path, []byte("original"), 0o644); err != nil {
		t.Fatal(err)
	}
	msg := materializeCmd(vfs.Disk{}, 1, path, "new content", 0, "r1", true, diskBaseline{})()
	e, ok := msg.(FileSaveErrorMsg)
	if !ok || !e.Conflict {
		t.Fatalf("expected conflict FileSaveErrorMsg, got %#v", msg)
	}
	if b, _ := os.ReadFile(path); string(b) != "original" {
		t.Fatalf("bind-new clobbered an existing file: %q", b)
	}
}

// TestMaterialize_OverwriteRefusesExternalChange: ⌘S must refuse to clobber a
// file that changed on disk since it was loaded (§1.4.7).
func TestMaterialize_OverwriteRefusesExternalChange(t *testing.T) {
	path := filepath.Join(t.TempDir(), "f.md")
	if err := os.WriteFile(path, []byte("v1"), 0o644); err != nil {
		t.Fatal(err)
	}
	base := baselineOf(vfs.Disk{}, path)
	// Simulate an external editor changing the file (different size → divergence).
	if err := os.WriteFile(path, []byte("v2 external longer"), 0o644); err != nil {
		t.Fatal(err)
	}
	msg := materializeCmd(vfs.Disk{}, 1, path, "mine", 0, "r1", false, base)()
	e, ok := msg.(FileSaveErrorMsg)
	if !ok || !e.Conflict {
		t.Fatalf("expected conflict FileSaveErrorMsg, got %#v", msg)
	}
	if b, _ := os.ReadFile(path); string(b) != "v2 external longer" {
		t.Fatalf("overwrite clobbered an external change: %q", b)
	}
}

// TestMaterialize_OverwriteWritesVerbatim: when the file matches its baseline,
// ⌘S writes the bytes verbatim — no line-ending/trailing-newline normalization
// (§1.4.5) — and reports the new baseline.
func TestMaterialize_OverwriteWritesVerbatim(t *testing.T) {
	path := filepath.Join(t.TempDir(), "f.md")
	if err := os.WriteFile(path, []byte("v1"), 0o644); err != nil {
		t.Fatal(err)
	}
	const want = "line1\r\nline2 no trailing nl"
	msg := materializeCmd(vfs.Disk{}, 1, path, want, 0, "r1", false, baselineOf(vfs.Disk{}, path))()
	saved, ok := msg.(FileSavedMsg)
	if !ok {
		t.Fatalf("expected FileSavedMsg, got %#v", msg)
	}
	if !saved.Baseline.valid {
		t.Fatal("expected a valid baseline on the save ack")
	}
	if b, _ := os.ReadFile(path); string(b) != want {
		t.Fatalf("bytes not written verbatim: %q", b)
	}
}

// TestRename_RefusesClobber: renaming onto an existing file must NOT destroy it.
func TestRename_RefusesClobber(t *testing.T) {
	dir := t.TempDir()
	a, b := filepath.Join(dir, "a.md"), filepath.Join(dir, "b.md")
	if err := os.WriteFile(a, []byte("A"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(b, []byte("B"), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, ok := fileRenameCmd(vfs.Disk{}, a, b)().(FileRenameErrorMsg); !ok {
		t.Fatalf("expected FileRenameErrorMsg when target exists")
	}
	if bb, _ := os.ReadFile(b); string(bb) != "B" {
		t.Fatalf("rename clobbered target: %q", bb)
	}
}

// TestRestore_OnlyNonEmptyGenuineScratch pins the launch-recovery scoping: a
// prior-session untitled with real content reopens, but a blank/whitespace one
// does NOT (it would otherwise clutter every launch). Combined with the docstate
// inode filter, this prevents the "stale/foreign Untitled tabs" regression.
func TestRestore_OnlyNonEmptyGenuineScratch(t *testing.T) {
	m := newTestWorkspace(t)
	store := docstate.NewTestStore(t)

	// Prior-session genuine non-empty untitled work — must be recovered.
	genuine, err := store.CreateScratch("note")
	if err != nil {
		t.Fatalf("CreateScratch genuine: %v", err)
	}
	if _, err := store.CreateSnapshot(genuine.ID, "recovered note text", "local", 0); err != nil {
		t.Fatalf("CreateSnapshot genuine: %v", err)
	}
	// Prior-session blank untitled (whitespace only) — must NOT resurface.
	blank, err := store.CreateScratch("blank")
	if err != nil {
		t.Fatalf("CreateScratch blank: %v", err)
	}
	if _, err := store.CreateSnapshot(blank.ID, "   \n\t", "local", 0); err != nil {
		t.Fatalf("CreateSnapshot blank: %v", err)
	}

	m, cmd := m.Update(StoreReadyMsg{Store: store})
	for _, msg := range execCmds(cmd) {
		m, _ = m.Update(msg)
	}

	if !tabHasDocID(m, genuine.ID) {
		t.Error("non-empty prior-session scratch was not recovered as a tab")
	}
	if tabHasDocID(m, blank.ID) {
		t.Error("blank prior-session scratch was wrongly recovered as a tab")
	}
}

// TestStartupUntitled_DurableAfterStoreReady pins Fix #4: the startup untitled
// (created before the store opened) gets a durable VFS doc once StoreReadyMsg
// arrives, so typed content is journaled and reconstructable — a crash no
// longer loses the session.
func TestStartupUntitled_DurableAfterStoreReady(t *testing.T) {
	m := newTestWorkspace(t)
	m = withStore(t, m)
	if m.view.DocID() == 0 {
		t.Fatal("startup untitled was not upgraded to a durable VFS doc")
	}
	m = focusEditor(m)
	m, _ = m.Update(tea.KeyPressMsg{Code: 'x', Text: "x"})
	m, _ = m.Update(tea.KeyPressMsg{Code: 'y', Text: "y"})

	got, err := m.store.RecoverDocument(m.view.DocID())
	if err != nil {
		t.Fatalf("RecoverDocument: %v", err)
	}
	if got == "" {
		t.Fatal("typed content was not journaled to the VFS")
	}
	if got != m.editor.Content() {
		t.Fatalf("VFS content %q != buffer %q", got, m.editor.Content())
	}
}
