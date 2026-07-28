package workspace

import (
	"os"
	"path/filepath"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/docstate"
	"rune/pkg/vfs"
)

// TestRecoverUnsavedEdits_AcrossStoreRestart is the §1.4.3 end-to-end
// guarantee, previously covered only at the docstate layer: an unsaved,
// journaled edit must survive a full store close + reopen (the crash/relaunch
// path) THROUGH the workspace's own real load path — recovered content
// displayed, doc dirty, the user's file on disk untouched, and the recovered
// buffer immediately editable (a fresh edit + ⌘Z round-trips).
//
// Session 2 reopens docstate.OpenAt on the same directory with the liveness
// check forced false (probe_test.go's relaunch pattern: both "sessions" share
// this test process's pid, so session 1 must be modeled as dead the way a
// real crashed process's pid would be).
func TestRecoverUnsavedEdits_AcrossStoreRestart(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	const original = "hello"
	if err := os.WriteFile(path, []byte(original), 0o644); err != nil {
		t.Fatal(err)
	}

	// Session 1: load, type one journaled char, never save, "crash" (close).
	m := withStoreAt(t, newTestWorkspace(t), dir)
	m = loadFile(m, path, original)
	docID := m.view.DocID()
	if docID == 0 {
		t.Fatal("store not available")
	}
	m = focusEditor(m)
	m = typeChar(m, 'X')
	const edited = "Xhello"
	if got := m.editor.Content(); got != edited {
		t.Fatalf("setup: editor content = %q, want %q", got, edited)
	}
	if !m.opentabs.HasDirty() {
		t.Fatal("setup: expected dirty after the unsaved edit")
	}
	if err := m.store.Close(); err != nil {
		t.Fatalf("close session 1 store: %v", err)
	}

	// Session 2: a brand-new workspace over the same recovery dir and disk.
	m2 := newTestWorkspace(t)
	m2 = m2.WithFS(vfs.Disk{})
	store2, _, err := docstate.OpenAt(dir)
	if err != nil {
		t.Fatalf("reopen store: %v", err)
	}
	t.Cleanup(func() { store2.Close() })
	store2.SetLivenessCheck(func(pid int, startedAt string) bool { return false }) // session 1 is dead
	store2.UseFS(m2.fsys())
	m2, cmd := m2.Update(StoreReadyMsg{Store: store2})
	m2 = settle(t, m2, cmd)

	// Load through the REAL path (loadFile drives store.Load + the
	// generation handshake; the disk file already holds `original`, so
	// nothing is rewritten and the identity survives).
	m2 = loadFile(m2, path, original)
	if m2.view.DocID() == 0 {
		t.Fatal("session 2: store not available")
	}

	// §1.4.3: the unsaved edit is recovered and displayed...
	if got := m2.editor.Content(); got != edited {
		t.Fatalf("recovered content = %q, want the crashed session's unsaved edit %q", got, edited)
	}
	// ...the doc is dirty (buffer ahead of disk)...
	if !m2.opentabs.HasDirty() {
		t.Fatal("recovered doc must be dirty — its unsaved edit is not on disk")
	}
	// ...and the user's file was NOT touched by recovery (§1.4.2: unsaved
	// work goes to the recovery store, never the user's file).
	disk, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if string(disk) != original {
		t.Fatalf("recovery modified the user's file: disk = %q, want untouched %q", disk, original)
	}

	// The recovered buffer is immediately EDITABLE: a fresh keystroke lands
	// and a real ⌘Z steps back to the recovered state.
	m2 = focusEditor(m2)
	m2 = typeChar(m2, 'Y')
	if got := m2.editor.Content(); got != "Y"+edited {
		t.Fatalf("post-recovery edit: content = %q, want %q", got, "Y"+edited)
	}
	m2, cmd = m2.Update(tea.KeyPressMsg{Code: 'z', Mod: tea.ModSuper})
	m2 = settle(t, m2, cmd)
	if got := m2.editor.Content(); got != edited {
		t.Fatalf("⌘Z after recovery: content = %q, want back to the recovered %q", got, edited)
	}
}
