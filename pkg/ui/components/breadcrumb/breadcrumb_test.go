package breadcrumb

import (
	"strings"
	"testing"

	"rune/pkg/ui/styles"
)

func newTestModel(path string) Model {
	m := New(styles.Default(), nil)
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
	view := m.View()
	if !strings.Contains(view, "(no file)") {
		t.Errorf("expected '(no file)' in view, got: %s", view)
	}
}

func TestBreadcrumb_DirOnly(t *testing.T) {
	m := New(styles.Default(), nil)
	m = m.SetDir("/workspace/notes")
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
