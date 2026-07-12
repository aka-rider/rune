package workspace

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/pages/workspace/mergemode"
	"rune/pkg/vfs"
)

// ─────────────────────────────────────────────────────────────────────────────
// G: passive "changed on disk" indicator — now driven by Load's own SyncState
// on tab-switch (free, no extra stat-on-focus round trip), and by probeDocCmd
// for the idle-detection paths (fileChangedMsg / dirChangedMsg / flush tick).
// ─────────────────────────────────────────────────────────────────────────────

// TestDiskChangedHint_ClearOnTabSwitchBack_AutoAdopted: switching back to a
// SyncDiskAhead tab (externally changed, no unsaved local edits) now adopts
// theirs as a REAL journaled transition (data-integrity-v4 remediation F1 —
// installDiskAhead), so the divergence the hint would otherwise warn about
// is immediately reconciled — store.Content(docID) tracks the displayed
// buffer, and the passive hint must NOT linger (nothing external is left
// unobserved). Pre-fix this asserted the OPPOSITE (hint stays true): that
// encoded the bug (F1) — the switch-back displayed theirs on screen but
// never adopted it, leaving the hint (correctly, given the unreconciled
// divergence) stuck, while the journal silently diverged from the buffer
// underneath (DL1).
func TestDiskChangedHint_ClearOnTabSwitchBack_AutoAdopted(t *testing.T) {
	dir := t.TempDir()
	pathA := filepath.Join(dir, "a.md")
	pathB := filepath.Join(dir, "b.md")
	if err := os.WriteFile(pathA, []byte("v1"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(pathB, []byte("b content"), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, pathA, "v1")
	docA := m.view.DocID()
	if docA == 0 {
		t.Fatal("store not available")
	}
	m = loadFile(m, pathB, "b content") // switch away from A

	// A changes externally while backgrounded.
	const externalContent = "v2 external change — longer"
	if err := os.WriteFile(pathA, []byte(externalContent), 0o644); err != nil {
		t.Fatal(err)
	}

	m, cmd := m.requestOpenPath(docA, pathA)
	m = settle(t, m, cmd)

	if m.diskChangedHint {
		t.Fatal("diskChangedHint: expected false after switching back — SyncDiskAhead auto-adopts theirs, reconciling the divergence")
	}
	if m.editor.Content() != externalContent {
		t.Fatalf("editor.Content() = %q, want the adopted external content %q", m.editor.Content(), externalContent)
	}
	vfsContent, err := m.store.Content(docA)
	if err != nil {
		t.Fatalf("store.Content: %v", err)
	}
	if vfsContent != externalContent {
		t.Fatalf("store.Content(docA) = %q, want %q (F1: the adoption must be journaled, not just displayed)", vfsContent, externalContent)
	}
}

func TestDiskChangedHint_ClearOnTabSwitchUnchanged(t *testing.T) {
	dir := t.TempDir()
	pathA := filepath.Join(dir, "a.md")
	pathB := filepath.Join(dir, "b.md")
	if err := os.WriteFile(pathA, []byte("stable"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(pathB, []byte("b content"), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, pathA, "stable")
	docA := m.view.DocID()
	if docA == 0 {
		t.Fatal("store not available")
	}
	m = loadFile(m, pathB, "b content")
	m.diskChangedHint = true // pre-set to confirm it gets cleared

	m, cmd := m.requestOpenPath(docA, pathA)
	m = settle(t, m, cmd)

	if m.diskChangedHint {
		t.Fatal("diskChangedHint: expected false after switching to an unchanged file")
	}
}

func TestFileChangedMsg_SetsHintForOpenFile(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	if err := os.WriteFile(path, []byte("v1"), 0o644); err != nil {
		t.Fatal(err)
	}
	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, path, "v1")
	if m.view.DocID() == 0 {
		t.Fatal("store not available")
	}

	if err := os.WriteFile(path, []byte("v2 external in-place edit — longer"), 0o644); err != nil {
		t.Fatal(err)
	}
	m, cmd := m.Update(fileChangedMsg{path: path})
	m = settle(t, m, cmd)
	if !m.diskChangedHint {
		t.Fatal("BUG1: fileChangedMsg for the open file must set diskChangedHint")
	}
}

func TestFileChangedMsg_IgnoresOtherFileAndOwnSave(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	other := filepath.Join(dir, "other.md")
	for _, p := range []string{path, other} {
		if err := os.WriteFile(p, []byte("v1"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, path, "v1")
	if m.view.DocID() == 0 {
		t.Fatal("store not available")
	}

	// A Write to the open file while OUR save is in flight is a self-write.
	// The in-flight save is a REAL one (startSaveGetAck physically performs
	// the atomic rewrite and holds the FileSavedMsg ack undelivered), so
	// activeSave carries the full identity startSave populates — including
	// SavedContent, without which SAVE-SM (settle's invariant sweep) trips.
	// The fileChangedMsg is then exactly what fsnotify would report for our
	// own publish: savingTarget compares the full identity (docID+path) and
	// must swallow it.
	m = focusEditor(m)
	m = typeChar(m, 'X') // dirty, so there is something real to save
	m, fsMsg := startSaveGetAck(t, m)
	m, cmd := m.Update(fileChangedMsg{path: path})
	m = settle(t, m, cmd)
	if m.diskChangedHint {
		t.Fatal("BUG1: a Write during our own in-flight save must not set the hint")
	}
	m, cmd = m.Update(fsMsg) // settle the save; hint must stay clear
	m = settle(t, m, cmd)
	if m.diskChangedHint {
		t.Fatal("BUG1: our own save ack must not set the hint")
	}

	// The OPEN file changed on disk, but the Write event names a DIFFERENT file.
	if err := os.WriteFile(path, []byte("v2 changed — longer"), 0o644); err != nil {
		t.Fatal(err)
	}
	m, cmd = m.Update(fileChangedMsg{path: other})
	m = settle(t, m, cmd)
	if m.diskChangedHint {
		t.Fatal("BUG1: a Write to a different file must not set the hint")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Fix C (BUG1): atomic-save external edits (temp→rename) must also surface
// the "changed on disk" indicator — not just fsnotify in-place Writes.
// ─────────────────────────────────────────────────────────────────────────────

func TestDirChangedMsg_AtomicSaveDivergence_SetsPersistentHint(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	if err := os.WriteFile(path, []byte("v1"), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, path, "v1")
	if m.view.DocID() == 0 {
		t.Fatal("store not available")
	}

	tmp := path + ".tmp-atomic"
	if err := os.WriteFile(tmp, []byte("v2 external ATOMIC save — longer"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Rename(tmp, path); err != nil {
		t.Fatal(err)
	}

	m, cmd := m.Update(dirChangedMsg{})
	m = settle(t, m, cmd)

	if !m.diskChangedHint {
		t.Fatal("BUG1: dirChangedMsg must detect the atomic-save divergence and set diskChangedHint")
	}
	view := m.footer.View()
	if !strings.Contains(view, "File changed on disk") {
		t.Errorf("BUG1: footer must render the persistent changed-on-disk indicator: %q", view)
	}
}

func TestDirChangedMsg_NoDivergence_NoHint(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	if err := os.WriteFile(path, []byte("stable"), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, path, "stable")

	m, cmd := m.Update(dirChangedMsg{})
	m = settle(t, m, cmd)

	if m.diskChangedHint {
		t.Fatal("dirChangedMsg with no real divergence must not set diskChangedHint")
	}
}

func TestHandleFileSavedMsg_ClearsDiskChangedHint(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	if err := os.WriteFile(path, []byte("hello"), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, path, "hello")
	m = m.setDiskChangedHint(true) // pre-set, as if a prior divergence was flagged
	m = focusEditor(m)

	m, saveCmd := m.startSave()
	if saveCmd == nil {
		t.Fatal("expected a materialize cmd")
	}
	result := saveCmd()
	savedMsg, ok := result.(FileSavedMsg)
	if !ok {
		t.Fatalf("expected FileSavedMsg, got %T: %v", result, result)
	}
	m, _, _ = m.handleFileSavedMsg(savedMsg, nil)

	if m.diskChangedHint {
		t.Fatal("BUG1: a successful save must clear the persistent diskChangedHint")
	}
	if strings.Contains(m.footer.View(), "File changed on disk") {
		t.Errorf("BUG1: footer must not still show the changed-on-disk indicator after a save: %q", m.footer.View())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Fix D (BUG2): read-only ours-vs-theirs preview at the [S]/[D]/[M] guard.
// raiseConflictGuard is now synchronous (pure SQLite) — no async read needed.
// ─────────────────────────────────────────────────────────────────────────────

func TestConflictGuard_PreviewRendersDiffAtGuardTime(t *testing.T) {
	const oursContent = "shared\nours changed\n"
	const theirsContent = "shared\ntheirs changed\n"

	// docID==0: raiseConflictGuard's ancestor-fetch (via Sync) is skipped, so
	// MergeHunks sees an EMPTY ancestor — both sides diverge from it and from
	// each other, guaranteeing a genuine 2-way conflict (not a clean
	// auto-merge) so the preview must show BOTH sides.
	m := withStore(t, newTestWorkspace(t))
	theirsHash, err := m.store.PutBlob(theirsContent)
	if err != nil {
		t.Fatal(err)
	}
	m, _ = m.raiseConflictGuard(0, "/fake/path.md", oursContent, theirsHash, 0)

	if !m.footer.InGuard() || m.footer.GuardKind() != footer.GuardMerge {
		t.Fatal("setup: expected GuardMerge raised")
	}
	if !m.guard.conflict.active {
		t.Fatal("setup: expected pendingConflict active")
	}
	if mergemode.IsActive(m.merge) {
		t.Fatal("BUG2: the guard-time preview must NOT activate the resolver — only [M] does")
	}

	body := m.View().Content
	if !strings.Contains(body, "ours changed") || !strings.Contains(body, "theirs changed") {
		t.Fatalf("BUG2: guard-time preview must show the ours-vs-theirs diff:\n%s", body)
	}
}

func TestConflictGuard_PreviewClearedOnEsc(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	const oursContent = "shared\nours changed\n"
	const theirsContent = "shared\ntheirs changed\n"
	if err := os.WriteFile(path, []byte(oursContent), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, path, oursContent)
	docID := m.view.DocID()

	theirsHash, err := m.store.PutBlob(theirsContent)
	if err != nil {
		t.Fatal(err)
	}
	m, _ = m.raiseConflictGuard(docID, path, oursContent, theirsHash, 0)
	if !m.guard.conflict.active {
		t.Fatal("setup: expected pendingConflict active")
	}

	m, _ = m.Update(footer.DataLossGuardResponseMsg{Response: footer.DataLossCancel})

	if m.guard.conflict.active {
		t.Fatal("Esc must clear pendingConflict")
	}
	body := m.View().Content
	if strings.Contains(body, "theirs changed") {
		t.Errorf("Esc must clear the guard-time preview — the diff must not linger:\n%s", body)
	}
}
