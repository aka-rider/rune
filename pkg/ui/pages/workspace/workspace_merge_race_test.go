package workspace

// Fix A verification: [M]/[D] must act on a FRESH probe of disk state taken at
// the action, never bytes cached at conflict-DETECTION time (the merge
// data-race / TOCTOU, §0). WP5: resolveProbeCmd (store.Probe) replaces the
// pre-v4 raw fsys.ReadFile mergeReadCmd.

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/pages/workspace/mergemode"
	"rune/pkg/vfs"
)

// TestMergeAction_UsesFreshTheirs_NotDetectionTimeCache: the file changes
// AGAIN, for real, between conflict detection and the [M] press. The merge
// must run against the LATEST disk bytes (a fresh Probe at the action), not
// whatever was on disk at detection — otherwise it silently merges/recreates
// stale content the user never actually chose (the merge data-race the plan
// calls "the core issue"). WP5: pendingConflict no longer caches theirs at
// all (it's re-probed fresh on every [M]/[D]), so this is a structural
// guarantee rather than a cache-invalidation one — this test pins it by
// changing disk TWICE and asserting only the LATEST write is ever visible.
func TestMergeAction_UsesFreshTheirs_NotDetectionTimeCache(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	const ancestorContent = "shared\noriginal\n"
	const oursContent = "shared\nours changed\n"
	const detectionTimeTheirs = "shared\ntheirs-at-detection\n"
	const freshTheirs = "shared\ntheirs-AFTER-detection\n"

	if err := os.WriteFile(path, []byte(ancestorContent), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, path, ancestorContent)
	docID := m.view.DocID()
	if docID == 0 {
		t.Fatal("store not available")
	}
	// A REAL journaled edit diverges ours from the ancestor — a genuine
	// two-way conflict.
	m = diverge(t, m, docID, ancestorContent, oursContent)

	// Disk holds the DETECTION-time content when the guard is raised...
	if err := os.WriteFile(path, []byte(detectionTimeTheirs), 0o644); err != nil {
		t.Fatal(err)
	}

	m.guard.conflict = conflictIntent{active: true, path: path, docID: docID}
	m = m.raiseGuardPrompt(guardConflict) // A3: keep guard.kind/phase coherent with the hand-set intent (kind-first dispatch reads guard.kind now)

	// ...but disk changes AGAIN for real before the user presses [M].
	if err := os.WriteFile(path, []byte(freshTheirs), 0o644); err != nil {
		t.Fatal(err)
	}

	m = runMergeAction(t, m, footer.DataLossMerge)

	if !mergemode.IsActive(m.merge) || !mergemode.HasUnresolvedConflicts(m.merge) {
		t.Fatal("expected an unresolved conflict after [M]")
	}
	view := m.merge.View()
	if strings.Contains(view, "theirs-at-detection") {
		t.Errorf("merge ran against the STALE detection-time cache, not fresh disk:\n%s", view)
	}
	if !strings.Contains(view, "theirs-AFTER-detection") {
		t.Errorf("merge must show the FRESH disk content read at the [M] press:\n%s", view)
	}

	// ResolveAdopt must have advanced the CAS baseline (saved_obs) to the
	// FRESH bytes just probed, not the stale detection-time cache — a
	// resolved-merge ⌘S would otherwise refuse (CAS mismatch) or, worse,
	// silently target the wrong disk fact.
	savedObs, hasSaved, err := m.store.SavedObs(docID)
	if err != nil || !hasSaved {
		t.Fatalf("SavedObs: hasSaved=%v err=%v", hasSaved, err)
	}
	anchor, err := m.store.GetBlob(savedObs.BlobHash)
	if err != nil {
		t.Fatalf("GetBlob(saved_obs): %v", err)
	}
	if anchor != freshTheirs {
		t.Fatalf("CAS baseline (saved_obs)=%q, want the FRESH theirs %q", anchor, freshTheirs)
	}
}

// TestMergeAction_FileDeletedMidFlight_RoutesToDeletedGuard_NoMergeNoWrite:
// the guard is up, then the file is DELETED from disk before [M] is pressed.
// [M] must not merge against (or recreate) the vanished file — it must route
// to the deleted guard instead (Fix A: detect deletion EARLY, at the action,
// not late at ⌘S).
func TestMergeAction_FileDeletedMidFlight_RoutesToDeletedGuard_NoMergeNoWrite(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	const oursContent = "shared\nours changed\n"
	const theirsContent = "shared\ntheirs changed\n"
	if err := os.WriteFile(path, []byte(theirsContent), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, path, oursContent)
	docID := m.view.DocID()
	if docID == 0 {
		t.Fatal("store not available")
	}

	m.guard.conflict = conflictIntent{active: true, path: path, docID: docID}
	m = m.raiseGuardPrompt(guardConflict) // A3: keep guard.kind/phase coherent with the hand-set intent (kind-first dispatch reads guard.kind now)

	// The file is deleted for real between the guard raise and the [M] press.
	if err := os.Remove(path); err != nil {
		t.Fatal(err)
	}

	m = runMergeAction(t, m, footer.DataLossMerge)

	if mergemode.IsActive(m.merge) {
		t.Fatal("race: [M] must NOT enter the resolver against a deleted file")
	}
	if !m.footer.InGuard() || m.footer.GuardKind() != footer.GuardDeleted {
		t.Fatalf("race: expected GuardDeleted raised (early detection); InGuard=%v kind=%v",
			m.footer.InGuard(), m.footer.GuardKind())
	}
	if !m.guard.deleted.active {
		t.Fatal("race: pendingDeleted must be armed")
	}
	if m.guard.conflict.active {
		t.Fatal("race: pendingConflict must be cleared")
	}
	// The buffer must be untouched — no merge ran against stale bytes, and
	// nothing was written back to a path that no longer exists.
	if got := m.editor.Content(); got != oursContent {
		t.Fatalf("race: buffer=%q must be untouched (still live ours), want %q", got, oursContent)
	}
	if _, err := os.Stat(path); err == nil {
		t.Fatal("race: the deleted file must NOT have been recreated by the merge action")
	}
}

// TestDiscardAction_FileDeletedMidFlight_RoutesToDeletedGuard mirrors the
// merge case for [D]iscard: a deletion mid-flight must never load a
// nonexistent "theirs" into the buffer.
func TestDiscardAction_FileDeletedMidFlight_RoutesToDeletedGuard(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	const oursContent = "shared\nours changed\n"
	const theirsContent = "shared\ntheirs changed\n"
	if err := os.WriteFile(path, []byte(theirsContent), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, path, oursContent)
	docID := m.view.DocID()
	if docID == 0 {
		t.Fatal("store not available")
	}

	m.guard.conflict = conflictIntent{active: true, path: path, docID: docID}
	m = m.raiseGuardPrompt(guardConflict) // A3: keep guard.kind/phase coherent with the hand-set intent (kind-first dispatch reads guard.kind now)

	if err := os.Remove(path); err != nil {
		t.Fatal(err)
	}

	m = runMergeAction(t, m, footer.DataLossDiscard)

	if !m.footer.InGuard() || m.footer.GuardKind() != footer.GuardDeleted {
		t.Fatalf("race: expected GuardDeleted raised; InGuard=%v kind=%v", m.footer.InGuard(), m.footer.GuardKind())
	}
	if got := m.editor.Content(); got != oursContent {
		t.Fatalf("race: [D]iscard must not load a vanished theirs; buffer=%q, want %q", got, oursContent)
	}
}
