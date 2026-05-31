package breadcrumb

import (
	"strings"
	"testing"

	"rune/pkg/ui/styles"
)

func newTestModel(path string) Model {
	m := New(styles.Default(), nil)
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
	m := New(styles.Default(), nil)
	m = m.SetSize(100, 1)
	m = m.SetUntitledName("Untitled.md")
	view := m.View()
	if !strings.Contains(view, "Untitled.md") {
		t.Errorf("expected 'Untitled.md' in view, got: %s", view)
	}
}

func TestBreadcrumb_DirOnly(t *testing.T) {
	m := New(styles.Default(), nil)
	m = m.SetSize(100, 1)
	m = m.SetDir("/workspace/notes")
	m = m.SetUntitledName("Untitled.md")
	view := m.View()
	if !strings.Contains(view, "notes") {
		t.Errorf("expected directory in view, got: %s", view)
	}
}

func TestBreadcrumb_SetPathMsg(t *testing.T) {
	m := newTestModel("/a/b/old.md")
	m, _ = m.Update(SetPathMsg{Path: "/a/b/new.md"})
	view := m.View()
	if !strings.Contains(view, "new.md") {
		t.Errorf("expected 'new.md' after SetPathMsg, got: %s", view)
	}
}
