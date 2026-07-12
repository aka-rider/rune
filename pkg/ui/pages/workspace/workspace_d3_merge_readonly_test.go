package workspace

import (
	"os"
	"path/filepath"
	"testing"

	"rune/pkg/ui/components/footer"
	"rune/pkg/vfs"
)

// TestDiscardConflictReadOnlyEditorRefusesAdoption is D3's regression test:
// applyDiscardConflict used to call resolveAdoptAt UNCONDITIONALLY after
// DrainEdits, without the journalEditOK gate its sibling installDiskAhead
// uses. On a read-only editor, ReplaceAll silently no-ops (textedit's own
// readOnly guard), so the buffer keeps ours — but the CAS baseline would
// still advance to theirs, blessing a later save that clobbers theirs while
// claiming the discard already reconciled it. The fix refuses the adoption
// (and surfaces an error) whenever the buffer install produced no edits.
func TestDiscardConflictReadOnlyEditorRefusesAdoption(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	if err := os.WriteFile(path, []byte("ours\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, path, "ours\n")
	docID := m.view.DocID()
	m = focusEditor(m)

	baselineBefore, hasBaseline, err := m.store.SavedObs(docID)
	if err != nil || !hasBaseline {
		t.Fatalf("setup: SavedObs: ok=%v err=%v", hasBaseline, err)
	}

	// Simulate the buffer-install being refused (mirrors installDiskAhead's
	// own "editor rejected the install" scenario) by making the editor
	// read-only right before the discard fires.
	m.editor = m.editor.SetReadOnly(true)

	if err := os.WriteFile(path, []byte("theirs on disk\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	m.guard.conflict = conflictIntent{active: true, path: path, docID: docID}
	m = m.raiseGuardPrompt(guardConflict) // A3: keep guard.kind/phase coherent with the hand-set intent (kind-first dispatch reads guard.kind now)
	m, cmd := m.Update(footer.DataLossGuardResponseMsg{Response: footer.DataLossDiscard})
	// The read-only editor refuses the buffer install, so applyDiscardConflict
	// returns early before ever journaling an edit — there is no autosave
	// flush (or anything else) for a full drainCmd to run into here, so this
	// doesn't need settleOneHop's one-hop stop.
	m = settle(t, m, cmd)

	if got := m.editor.Content(); got != "ours\n" {
		t.Fatalf("buffer changed despite a read-only editor: got %q, want %q", got, "ours\n")
	}

	afterBaseline, hasAfter, err := m.store.SavedObs(docID)
	if err != nil || !hasAfter {
		t.Fatalf("SavedObs after discard: ok=%v err=%v", hasAfter, err)
	}
	if afterBaseline.ID != baselineBefore.ID {
		t.Fatalf("D3 (must fail pre-fix): CAS baseline advanced (%d -> %d) despite the read-only editor refusing the buffer install — a later save could clobber theirs while believing the discard already reconciled it",
			baselineBefore.ID, afterBaseline.ID)
	}
}

// TestMergeConflictReadOnlyEditorRefusesAdoption is applyMergeConflict's
// half of D3's regression test — same gate, entered via [M]erge instead of
// [D]iscard.
func TestMergeConflictReadOnlyEditorRefusesAdoption(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	const ancestorContent = "shared\noriginal\n"
	const oursContent = "shared\nours changed\n"
	const theirsContent = "shared\ntheirs changed\n"
	if err := os.WriteFile(path, []byte(ancestorContent), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, path, ancestorContent)
	docID := m.view.DocID()
	m = diverge(t, m, docID, ancestorContent, oursContent)

	baselineBefore, hasBaseline, err := m.store.SavedObs(docID)
	if err != nil || !hasBaseline {
		t.Fatalf("setup: SavedObs: ok=%v err=%v", hasBaseline, err)
	}

	m.editor = m.editor.SetReadOnly(true)

	if err := os.WriteFile(path, []byte(theirsContent), 0o644); err != nil {
		t.Fatal(err)
	}
	m.guard.conflict = conflictIntent{active: true, path: path, docID: docID}
	m = m.raiseGuardPrompt(guardConflict) // A3: keep guard.kind/phase coherent with the hand-set intent (kind-first dispatch reads guard.kind now)
	m, cmd := m.Update(footer.DataLossGuardResponseMsg{Response: footer.DataLossMerge})
	// Same reasoning as the discard case above: the read-only editor refuses
	// the marker-buffer install, so applyMergeConflict returns early before
	// journaling anything — a full drainCmd has nothing extra to settle.
	m = settle(t, m, cmd)

	if got := m.editor.Content(); got != oursContent {
		t.Fatalf("buffer changed despite a read-only editor: got %q, want %q", got, oursContent)
	}

	afterBaseline, hasAfter, err := m.store.SavedObs(docID)
	if err != nil || !hasAfter {
		t.Fatalf("SavedObs after merge: ok=%v err=%v", hasAfter, err)
	}
	if afterBaseline.ID != baselineBefore.ID {
		t.Fatalf("D3 (must fail pre-fix): CAS baseline advanced (%d -> %d) despite the read-only editor refusing the marker-buffer install",
			baselineBefore.ID, afterBaseline.ID)
	}
}
