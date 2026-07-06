package opentabs

import "testing"

// TestRenameFile_UpdatesPathAndName pins RenameFile's contract: the tab
// matching oldPath gets its path and display name updated to newPath, and no
// other tab is touched.
func TestRenameFile_UpdatesPathAndName(t *testing.T) {
	m := newModel()
	m = m.OpenFile(1, "old.md")
	m = m.OpenFile(2, "other.md")

	var ok bool
	m, ok = m.RenameFile("old.md", "renamed.md")

	if !ok {
		t.Fatal("RenameFile ok = false, want true (no collision)")
	}
	if got := m.PathAt(0); got != "renamed.md" {
		t.Fatalf("PathAt(0) = %q, want %q", got, "renamed.md")
	}
	if got := m.tabs[0].Name; got != tabName("renamed.md") {
		t.Fatalf("tab name = %q, want %q", got, tabName("renamed.md"))
	}
	if got := m.PathAt(1); got != "other.md" {
		t.Fatalf("unrelated tab must be untouched: PathAt(1) = %q, want %q", got, "other.md")
	}
}

// TestRenameFile_NoMatchIsNoop: renaming a path with no matching tab changes nothing.
func TestRenameFile_NoMatchIsNoop(t *testing.T) {
	m := newModel()
	m = m.OpenFile(1, "a.md")

	var ok bool
	m, ok = m.RenameFile("nonexistent.md", "b.md")

	if !ok {
		t.Fatal("RenameFile ok = false, want true (no matching tab is a legitimate no-op)")
	}
	if got := m.PathAt(0); got != "a.md" {
		t.Fatalf("PathAt(0) = %q, want unchanged %q", got, "a.md")
	}
}

// TestRenameFile_CollisionDetachesOther pins the T1/EDITOR-TAB-COH fix
// (FuzzHumanSession 5e743dba2a60dff6): renaming onto a path a DIFFERENT tab
// already has open must not create two tabs with the same path (T1) NOR
// leave the renaming tab's displayed path stale (EDITOR-TAB-COH demands the
// active tab's Path always matches reality) — disk truth wins: the renaming
// tab takes newPath, and the colliding tab is DETACHED (Path cleared to "",
// content/DocID/dirty-state untouched) rather than clobbered or duplicated.
func TestRenameFile_CollisionDetachesOther(t *testing.T) {
	m := newModel()
	m = m.OpenFile(1, "a.md")
	m = m.OpenFile(2, "d.md")

	var ok bool
	m, ok = m.RenameFile("a.md", "d.md")

	if ok {
		t.Fatal("RenameFile ok = true, want false (a collision was reconciled)")
	}
	if got := m.PathAt(0); got != "d.md" {
		t.Fatalf("PathAt(0) = %q, want %q (disk truth: this doc is what's at d.md now)", got, "d.md")
	}
	if got := m.PathAt(1); got != "" {
		t.Fatalf("PathAt(1) = %q, want \"\" (colliding tab detached, not duplicated or clobbered)", got)
	}
	if m.tabs[1].DocID != 2 {
		t.Fatalf("detached tab's DocID = %d, want unchanged 2 (content/identity preserved)", m.tabs[1].DocID)
	}
}
