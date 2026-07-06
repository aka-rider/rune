package breadcrumb

import (
	"strings"
	"testing"

	"rune/pkg/ui/styles"
)

func newTestModel(path string) Model {
	m := New(styles.Default())
	m = m.SetSize(100, 1) // Give it enough width so it doesn't truncate to ellipsis
	m = m.SetPath(path)
	return m
}

func TestBreadcrumb_DisplayPath(t *testing.T) {
	m := newTestModel("/a/b/note.md")
	view := m.View()
	if !strings.Contains(view, "note.md") {
		t.Errorf("expected view to contain 'note.md', got: %s", view)
	}
}

func TestBreadcrumb_NoFile(t *testing.T) {
	m := New(styles.Default())
	m = m.SetSize(100, 1)
	view := m.View()
	if !strings.Contains(view, "Untitled.md") {
		t.Errorf("expected 'Untitled.md' in view, got: %s", view)
	}
}

func TestBreadcrumb_DirOnly(t *testing.T) {
	m := New(styles.Default())
	m = m.SetSize(100, 1)
	m = m.SetDir("/workspace/notes")
	view := m.View()
	if !strings.Contains(view, "notes") {
		t.Errorf("expected directory in view, got: %s", view)
	}
}

// TestBreadcrumb_SiblingPathNotClaimedAsUnderRoot is B3's regression test: a
// bare strings.HasPrefix(path, root) has no separator boundary, so root
// "/a/vault" would wrongly claim "/a/vault2/notes.md" as living under it
// (the remainder "2/notes.md" doesn't start with a path separator). The
// fallback path (rendered as the raw absolute path, not falsely relativized
// under "vault") must be used instead.
func TestBreadcrumb_SiblingPathNotClaimedAsUnderRoot(t *testing.T) {
	m := New(styles.Default())
	m = m.SetSize(100, 1)
	m = m.SetDir("/a/vault")
	m = m.SetPath("/a/vault2/notes.md")
	view := m.View()
	if strings.Contains(view, "vault/") || strings.Contains(view, "vault2") == false {
		t.Errorf("sibling path /a/vault2/notes.md was wrongly relativized under root /a/vault: %s", view)
	}
}

// TestBreadcrumb_ExactRootBoundaryStillRelativizes is the positive control
// for B3's fix: a genuine descendant of root (remainder starts with the path
// separator) must still relativize normally.
func TestBreadcrumb_ExactRootBoundaryStillRelativizes(t *testing.T) {
	m := New(styles.Default())
	m = m.SetSize(100, 1)
	m = m.SetDir("/a/vault")
	m = m.SetPath("/a/vault/notes.md")
	view := m.View()
	if !strings.Contains(view, "vault") || !strings.Contains(view, "notes.md") {
		t.Errorf("expected path under root to relativize under 'vault', got: %s", view)
	}
}
