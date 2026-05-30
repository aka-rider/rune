package breadcrumb

import (
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/styles"
)

// stubValidate rejects any string containing '/'.
func stubValidate(s string) error {
	if strings.ContainsRune(s, '/') {
		return &validationError{"character '/' is not allowed in filenames"}
	}
	return nil
}

type validationError struct{ msg string }

func (e *validationError) Error() string { return e.msg }

func newTestModel(path string) Model {
	m := New(styles.Default(), stubValidate)
	m = m.SetPath(path)
	return m
}

func TestBreadcrumb_EnterEditMode(t *testing.T) {
	m := newTestModel("/a/b/note.md")
	m = m.SetEditing(true)
	if !m.editing {
		t.Fatal("expected editing=true")
	}
	if !strings.Contains(m.View(), "note") {
		t.Errorf("expected view to contain stem 'note', got: %s", m.View())
	}
}

func TestBreadcrumb_TypePrintableChar(t *testing.T) {
	m := newTestModel("/a/b/note.md")
	m = m.SetEditing(true)
	msg := tea.KeyPressMsg{Text: "x"}
	m, _ = m.Update(msg)
	if m.draft != "notex" {
		t.Errorf("expected draft='notex', got %q", m.draft)
	}
	if m.draftErr != "" {
		t.Errorf("expected no error, got %q", m.draftErr)
	}
}

func TestBreadcrumb_TypeUnicodeChar(t *testing.T) {
	m := newTestModel("/a/b/note.md")
	m = m.SetEditing(true)
	msg := tea.KeyPressMsg{Text: "é"}
	m, _ = m.Update(msg)
	if !strings.HasSuffix(m.draft, "é") {
		t.Errorf("expected draft to end with 'é', got %q", m.draft)
	}
	if m.draftErr != "" {
		t.Errorf("expected no error, got %q", m.draftErr)
	}
}

func TestBreadcrumb_TypeReservedChar(t *testing.T) {
	m := newTestModel("/a/b/note.md")
	m = m.SetEditing(true)
	originalDraft := m.draft
	msg := tea.KeyPressMsg{Text: "/"}
	m, _ = m.Update(msg)
	if m.draft != originalDraft {
		t.Errorf("expected draft unchanged %q, got %q", originalDraft, m.draft)
	}
	if m.draftErr == "" {
		t.Error("expected draftErr to be set after reserved char")
	}
}

func TestBreadcrumb_Backspace(t *testing.T) {
	m := newTestModel("/a/b/abc.md")
	m = m.SetEditing(true)
	// draft is "abc"; send backspace
	msg := tea.KeyPressMsg{Code: tea.KeyBackspace}
	m, _ = m.Update(msg)
	if m.draft != "ab" {
		t.Errorf("expected draft='ab', got %q", m.draft)
	}
}

func TestBreadcrumb_Commit_Valid(t *testing.T) {
	m := newTestModel("/a/b/note.md")
	m = m.SetEditing(true)
	// draft is "note", no error
	msg := tea.KeyPressMsg{Code: tea.KeyEnter}
	var cmd tea.Cmd
	m, cmd = m.Update(msg)
	if m.editing {
		t.Error("expected editing=false after commit")
	}
	if cmd == nil {
		t.Fatal("expected a Cmd to be returned")
	}
	result := cmd()
	commit, ok := result.(TitleEditCommitMsg)
	if !ok {
		t.Fatalf("expected TitleEditCommitMsg, got %T", result)
	}
	if commit.Name != "note" {
		t.Errorf("expected Name='note', got %q", commit.Name)
	}
}

func TestBreadcrumb_Commit_BlockedWhenError(t *testing.T) {
	m := newTestModel("/a/b/note.md")
	m = m.SetEditing(true)
	// Force an error by typing a reserved char first (won't append, but sets err)
	// Let's set draftErr directly via a bad key
	msg := tea.KeyPressMsg{Text: "/"}
	m, _ = m.Update(msg)
	// draftErr should be set
	if m.draftErr == "" {
		t.Skip("expected draftErr to be set - validate may not reject '/'")
	}
	enterMsg := tea.KeyPressMsg{Code: tea.KeyEnter}
	_, cmd := m.Update(enterMsg)
	if cmd != nil {
		result := cmd()
		if _, ok := result.(TitleEditCommitMsg); ok {
			t.Error("expected no TitleEditCommitMsg when draftErr is set")
		}
	}
}

func TestBreadcrumb_Escape(t *testing.T) {
	m := newTestModel("/a/b/note.md")
	m = m.SetEditing(true)
	msg := tea.KeyPressMsg{Code: tea.KeyEscape}
	m, _ = m.Update(msg)
	if m.editing {
		t.Error("expected editing=false after Escape")
	}
	if m.draftErr != "" {
		t.Errorf("expected draftErr cleared, got %q", m.draftErr)
	}
}
