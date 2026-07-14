package workspace

import (
	"strings"
	"testing"

	"rune/pkg/ui/components/markdownedit"
)

// TestFinalizeProjectsDocPathToEditor locks in the review-#1 fix: the editor's
// resolution base is projected from the single source (m.view) at finalize — so a
// transition that changes the path WITHOUT reloading content (bind-new, rename)
// still updates it. Before the fix, only SetContent set it → it drifted.
func TestFinalizeProjectsDocPathToEditor(t *testing.T) {
	m := newTestWorkspace(t)
	// Simulate bind-new / rename: m.view changes, NO SetContent.
	m.view = fileView("/vault/sub/note.md", 7)
	m, _ = m.finalize(nil)
	if got := m.editor.DocPath(); got != "/vault/sub/note.md" {
		t.Errorf("editor docPath = %q after finalize; want it projected from m.view.Path()", got)
	}
}

// TestFollowMissingInternalLinkKeepsContent locks in the data-safety guard: a
// dead link (the editor resolves it to LinkMissing) must NOT clear the editor —
// requestOpenPath is never called, so the open file stays shown and a later ⌘S
// can't overwrite it with "". The dead link is reported in the footer instead.
func TestFollowMissingInternalLinkKeepsContent(t *testing.T) {
	m := newTestWorkspace(t)
	m.editor, _ = m.editor.SetContent("# Real content\nstays put\n")
	m.view = fileView("/some/note.md", 0)
	before := m.editor.Content()

	m, cmd := m.Update(markdownedit.LinkActivatedMsg{Kind: markdownedit.LinkMissing, Raw: "missing.md"})

	if got := m.editor.Content(); got != before {
		t.Errorf("editor content changed on broken-link follow:\n before: %q\n after:  %q", before, got)
	}
	if m.view.Path() != "/some/note.md" {
		t.Errorf("view path changed on broken-link follow: %q", m.view.Path())
	}
	if v := m.footer.View(); !strings.Contains(v, "not found") || !strings.Contains(v, "missing.md") {
		t.Errorf("expected a 'Link not found: missing.md' footer error; View=%q", v)
	}
	_ = cmd
}
