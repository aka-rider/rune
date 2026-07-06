package workspace

import (
	"os"
	"path/filepath"
	"testing"

	"rune/pkg/docstate"
	"rune/pkg/editor/buffer"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/pages/workspace/mergemode"
	"rune/pkg/vfs"
)

// ─────────────────────────────────────────────────────────────────────────────
// Load-time / crash-recovery conflict guard (plan step 1)
// ─────────────────────────────────────────────────────────────────────────────

// setupLoadConflict writes ancestor to disk, loads it (stamping the disk-fact
// observation + recovery anchor), journals ours DIRECTLY (simulating unsaved
// edits surviving a crash — a real AppendEdit, exactly what recovery replays),
// writes theirs to disk, and reloads so handleFileLoadedMsg's Sync-driven
// conflict detection engages. Returns the workspace after the conflict-
// triggering reload, the docID, and the path.
func setupLoadConflict(t *testing.T, ancestor, ours, theirs string) (Model, int64, string) {
	t.Helper()
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")

	if err := os.WriteFile(path, []byte(ancestor), 0o644); err != nil {
		t.Fatalf("setup: write ancestor: %v", err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})

	m = loadFile(m, path, ancestor)
	docID := m.view.DocID()
	if docID == 0 {
		t.Skip("store not available — docID is 0")
	}

	// Simulate a crash-recovered local edit: journal the whole-buffer
	// replacement ancestor→ours (mirrors a real ReplaceAll's AppliedEdit).
	// Deleted must be set — buffer.ReplayForward skips len(Deleted) bytes at
	// Start (not End-Start), so omitting it replays as a pure insert with the
	// ancestor's tail left concatenated on.
	if _, err := m.store.AppendEdit(docID,
		[]buffer.AppliedEdit{{Start: 0, End: len(ancestor), Deleted: ancestor, Insert: ours}}, nil, nil); err != nil {
		t.Fatalf("setup: journal ours: %v", err)
	}

	if err := os.WriteFile(path, []byte(theirs), 0o644); err != nil {
		t.Fatalf("setup: write theirs: %v", err)
	}

	m = loadFile(m, path, theirs)
	return m, docID, path
}

func TestLoadTimeConflict_RaisesGuard(t *testing.T) {
	const ancestor = "shared original content"
	const ours = "ours unsaved edits after crash"
	const theirs = "theirs external change on disk"

	m, _, _ := setupLoadConflict(t, ancestor, ours, theirs)

	if !m.pendingConflict.active {
		t.Fatal("load-time conflict: pendingConflict should be active")
	}
	if !m.footer.InGuard() || m.footer.GuardKind() != footer.GuardMerge {
		t.Fatalf("load-time conflict: footer guard must be GuardMerge; inGuard=%v kind=%v",
			m.footer.InGuard(), m.footer.GuardKind())
	}
	if got := m.editor.Content(); got != ours {
		t.Fatalf("load-time conflict: editor=%q, want ours=%q", got, ours)
	}
}

// TestB1_EscThenQuitSaveRefused: raise load-time conflict → DataLossCancel
// (Esc) → quit "Save all" must still refuse: Load already advanced saved_obs
// to theirs (the very sighting that revealed the divergence), so the CAS
// check alone would pass — startSave/saveAllDirtyForQuit's own fresh
// Sync-Diverged re-check (§1.4.8) is what blocks it (a genuine v4 design gap
// found and fixed during WP5 integration; see workspace_quit.go).
func TestB1_EscThenQuitSaveRefused(t *testing.T) {
	const ancestor = "initial content"
	const ours = "ours unsaved edits"
	const theirs = "theirs external change"

	m, docID, path := setupLoadConflict(t, ancestor, ours, theirs)

	if !m.pendingConflict.active {
		t.Fatal("B1: expected conflict guard raised on load")
	}

	m, _ = m.Update(footer.DataLossGuardResponseMsg{Response: footer.DataLossCancel})
	if m.pendingConflict.active {
		t.Fatal("B1: Esc must clear pendingConflict (step 5)")
	}

	m.opentabs = m.opentabs.MarkDirtyByID(docID)

	_, batchCmd := m.saveAllDirtyForQuit()
	if batchCmd != nil {
		result := batchCmd()
		if saved, ok := result.(FileSavedMsg); ok {
			t.Fatalf("B1: quit-save must not silently clobber theirs; got FileSavedMsg %#v", saved)
		}
	}

	diskContent, _ := os.ReadFile(path)
	if string(diskContent) != theirs {
		t.Fatalf("B1: disk clobbered; want %q, got %q", theirs, string(diskContent))
	}
}

// TestEscThenSave_ReRaisesConflict: Esc → ⌘S must re-detect the unresolved
// divergence (never silent clobber after Esc).
func TestEscThenSave_ReRaisesConflict(t *testing.T) {
	const ancestor = "original content"
	const ours = "ours edits"
	const theirs = "theirs external version"

	m, _, path := setupLoadConflict(t, ancestor, ours, theirs)

	m, _ = m.Update(footer.DataLossGuardResponseMsg{Response: footer.DataLossCancel})
	m = focusEditor(m)

	m, saveCmd := m.startSave()
	if m.pendingConflict.active == false && saveCmd != nil {
		result := saveCmd()
		if _, ok := result.(FileSavedMsg); ok {
			t.Fatal("Esc-then-⌘S: must not silently write over an unresolved conflict")
		}
	}
	if !m.pendingConflict.active {
		t.Fatal("Esc-then-⌘S: expected the conflict guard to be re-raised")
	}

	diskContent, _ := os.ReadFile(path)
	if string(diskContent) != theirs {
		t.Fatalf("Esc-then-⌘S: disk clobbered; want %q, got %q", theirs, string(diskContent))
	}
}

func TestLoadTimeNoConflict_DiskEqualsAncestor(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	const content = "unchanged disk content"

	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, path, content)
	docID := m.view.DocID()
	if docID == 0 {
		t.Skip("store not available")
	}

	m = loadFile(m, path, content) // reload, unchanged

	if m.pendingConflict.active {
		t.Fatal("no-change reload: pendingConflict must not be raised (false positive)")
	}
	if m.footer.InGuard() {
		t.Fatal("no-change reload: guard must not be raised (false positive)")
	}
}

// TestLoadTimeNoConflict_OursEqualsAncestor (R1): when the disk changed
// externally but ours equals the ancestor (no unsaved local edits), the buffer
// must be updated to theirs with no conflict guard.
func TestLoadTimeNoConflict_OursEqualsAncestor(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	const ancestor = "original content"
	const theirs = "externally changed content"

	if err := os.WriteFile(path, []byte(ancestor), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, path, ancestor)
	docID := m.view.DocID()
	if docID == 0 {
		t.Skip("store not available")
	}

	if err := os.WriteFile(path, []byte(theirs), 0o644); err != nil {
		t.Fatal(err)
	}
	m = loadFile(m, path, theirs)

	if m.pendingConflict.active {
		t.Fatal("R1: pendingConflict must not be raised when ours==ancestor (no unsaved edits)")
	}
	if m.footer.InGuard() {
		t.Fatal("R1: guard must not be raised when ours==ancestor")
	}
	if got := m.editor.Content(); got != theirs {
		t.Fatalf("R1: editor=%q, want theirs=%q (current disk)", got, theirs)
	}
}

// TestMerge_ResolveAdvancesSavedObs: after [M]erge, ResolveAdopt must advance
// saved_obs to theirs so a resolved-merge ⌘S writes cleanly without
// re-detecting divergence and looping the guard (replaces the pre-v4
// "baseline re-stamped" test — the v4 equivalent of a baseline IS saved_obs).
func TestMerge_ResolveAdvancesSavedObs(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	const theirsContent = "shared line\ntheirs version\n"

	if err := os.WriteFile(path, []byte(theirsContent), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, path, theirsContent)
	docID := m.view.DocID()
	if docID == 0 {
		t.Skip("store not available")
	}

	// ancestor == theirs → the 3-way merge auto-resolves cleanly (no true
	// conflicts): ours is the only changed side.
	const oursContent = "shared line\nours version\n"
	m.editor = m.editor.SetContent(oursContent)
	m.pendingConflict = pendingConflict{active: true, path: path, docID: docID}
	m = runMergeAction(t, m, footer.DataLossMerge)

	if m.pendingConflict.active {
		t.Fatal("[M]: pendingConflict still active after DataLossMerge")
	}
	sync, err := m.store.Sync(docID)
	if err != nil {
		t.Fatalf("Sync: %v", err)
	}
	if sync.Kind == docstate.SyncDiverged {
		t.Fatal("[M]: sync must not remain Diverged after ResolveAdopt (Claim B)")
	}

	if mergemode.IsActive(m.merge) && mergemode.HasUnresolvedConflicts(m.merge) {
		t.Skip("[M]: merge has unresolved conflicts — need clean merge for this path")
	}

	m = focusEditor(m)
	m, saveCmd := m.startSave()
	if saveCmd == nil {
		t.Fatal("[M] resolve: startSave must return a materialize cmd after ResolveAdopt")
	}
	if !m.activeSave.InFlight {
		t.Fatal("[M] resolve: save must be in flight")
	}
	result := saveCmd()
	if _, ok := result.(FileSavedMsg); !ok {
		t.Fatalf("[M] resolve → ⌘S: expected FileSavedMsg (no guard loop); got %T: %v", result, result)
	}
}
