package workspace

import (
	"os"
	"path/filepath"
	"testing"

	"rune/pkg/docstate"
	"rune/pkg/ui/pages/workspace/mergemode"
	"rune/pkg/vfs"

	tea "charm.land/bubbletea/v2"
)

// TestR1Adopt_JournaledAndClean is the DL1 regression (F1), a deterministic
// replacement for the unreachable fuzz corpus entry 2e0bd2e4f177caf4 (a
// fresh worktree never carries prior fuzz-run artifacts — see the
// data-integrity-v4-remediation plan). Investigator-confirmed minimal
// sequence: open A -> switch away -> external write to A -> re-open -> the
// buffer AND the journal must agree, IMMEDIATELY, with no further keystroke
// needed. Pre-fix, handleFileLoadedMsg's SyncDiskAhead branch displayed
// theirs via a bare SetContent with no journaled adoption: store.Content(doc)
// silently kept reconstructing the STALE pre-external-change ancestor while
// the editor showed theirs, so a later quit/evict could write the stale
// reconstruction back over newer disk, and a second revisit could silently
// re-diverge.
func TestR1Adopt_JournaledAndClean(t *testing.T) {
	dir := t.TempDir()
	pathA := filepath.Join(dir, "a.md")
	pathB := filepath.Join(dir, "b.md")
	const originalA = "original A content\n"
	const externalA = "external A change — landed while backgrounded\n"

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, pathA, originalA)
	docA := m.view.DocID()
	if docA == 0 {
		t.Fatal("store not available")
	}
	m = loadFile(m, pathB, "b content\n") // switch away from A

	// A changes externally while backgrounded — no local edits ever made to A.
	if err := os.WriteFile(pathA, []byte(externalA), 0o644); err != nil {
		t.Fatal(err)
	}

	// Re-open A: SyncDiskAhead must auto-adopt as a REAL journaled transition.
	m, cmd := m.requestOpenPath(docA, pathA)
	m = settle(t, m, cmd)

	if got := m.editor.Content(); got != externalA {
		t.Fatalf("editor.Content() = %q, want the adopted external content %q", got, externalA)
	}
	vfsContent, err := m.store.Content(docA)
	if err != nil {
		t.Fatalf("store.Content: %v", err)
	}
	if vfsContent != m.editor.Content() {
		t.Fatalf("store.Content(docA) = %q != editor.Content() = %q — the adoption must be journaled, not just displayed (F1/DL1)",
			vfsContent, m.editor.Content())
	}
	dirty, err := m.store.IsDirty(docA)
	if err != nil {
		t.Fatalf("IsDirty: %v", err)
	}
	if dirty {
		t.Fatal("IsDirty = true immediately after a clean auto-adopt, want false")
	}

	// Switch away and back a SECOND time — must NOT silently revert (kills
	// the silent-revert half of F1): the first adoption is durable, so the
	// second reload finds nothing left to reconcile.
	m = loadFile(m, pathB, "b content\n")
	m, cmd2 := m.requestOpenPath(docA, pathA)
	m = settle(t, m, cmd2)
	if got := m.editor.Content(); got != externalA {
		t.Fatalf("second revisit: editor.Content() = %q, want still the adopted content %q (silent revert)", got, externalA)
	}
	if sync, err := m.store.Sync(docA); err != nil || sync.Kind != docstate.SyncClean {
		t.Fatalf("second revisit: Sync = %+v err=%v, want SyncClean (nothing left to adopt)", sync, err)
	}

	// Quit "Save all" must never write the stale pre-adopt reconstruction
	// over newer disk: A is clean (nothing to write), so saveAllDirtyForQuit
	// excludes it from the batch entirely, and disk stays exactly what was
	// adopted.
	_, batchCmd := m.saveAllDirtyForQuit()
	if batchCmd != nil {
		m = settle(t, m, batchCmd)
	}
	diskContent, err := os.ReadFile(pathA)
	if err != nil {
		t.Fatal(err)
	}
	if string(diskContent) != externalA {
		t.Fatalf("disk after quit-save = %q, want untouched %q (F1: never write the stale reconstruction over newer disk)",
			diskContent, externalA)
	}
}

// TestMergeAbort_ReraisesDiverged is the F3 regression: guard -> [M]erge ->
// Esc-abort must re-raise the Diverged conflict guard immediately (not
// silently leave a stale "resolved" CAS baseline a later save could clobber
// theirs with) and must never touch disk. Pre-fix, mergemode.Abort reverted
// the buffer via a FORWARD journaled edit with no store-side undo of the
// merge-entry adoption (resolveAdoptAt); the position-derived ancestor never
// re-exposed the divergence, so Sync silently read the doc as merely
// BufferAhead and a subsequent save would have clobbered theirs.
func TestMergeAbort_ReraisesDiverged(t *testing.T) {
	m, path, docID := enterRealConflict(t)

	m = m.setFocus(paneCenter)
	m, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyEscape})
	m = settle(t, m, cmd)

	if mergemode.IsActive(m.merge) {
		t.Fatal("Esc must abort and deactivate the merge")
	}
	if !m.footer.InGuard() {
		t.Fatal("F3: Esc-abort must re-raise the conflict guard immediately — the merge-entry CAS adoption must be abandoned and re-probed")
	}
	if !m.guard.conflict.active {
		t.Fatal("F3: Esc-abort must re-raise pendingConflict (GuardMerge), not some other guard")
	}

	sync, err := m.store.Sync(docID)
	if err != nil {
		t.Fatalf("Sync: %v", err)
	}
	if sync.Kind != docstate.SyncDiverged {
		t.Fatalf("Sync.Kind after Esc-abort = %v, want SyncDiverged", sync.Kind)
	}

	// Disk must be completely untouched by the abort.
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if string(data) != "shared\ntheirs changed\n" {
		t.Fatalf("disk mutated by Esc-abort: got %q", string(data))
	}
}
