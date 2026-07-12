package workspace

// Draft lifecycle end-to-end (§1.4.2): an untitled draft's unsaved content
// lives in the recovery store ONLY — no .md file may appear until the user
// names it; naming binds the SAME docID to the new file atomically; and
// discarding a draft through the real close guard never creates a file.

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/command"
	"rune/pkg/editor/keybind"
	"rune/pkg/terminal"
	"rune/pkg/ui/components/markdownedit"
	"rune/pkg/ui/components/title"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// newDraftWorkspace builds a workspace rooted at docsDir (so a bind-new
// materialize writes inside the test's own TempDir, not the cwd) with a REAL
// on-disk recovery store under a SEPARATE dir — keeping docsDir free of
// rune.db, so "no file appears in docsDir" is exactly the §1.4.2 assertion.
func newDraftWorkspace(t *testing.T, docsDir string) Model {
	t.Helper()
	keys := keymap.Default()
	st := styles.Default()
	builder := command.NewBuilder()
	builder, err := markdownedit.RegisterCommands(builder)
	if err != nil {
		t.Fatalf("register commands: %v", err)
	}
	reg := builder.Build()
	res, _ := keybind.NewResolver(nil)

	m := New(keys, st, reg, res, terminal.TermCaps{}, docsDir, nil).WithWatcher(NoopWatcher{})
	m, _ = m.Update(tea.WindowSizeMsg{Width: 80, Height: 24})
	return withStoreAt(t, m, t.TempDir())
}

// mdFiles lists the .md files currently present under dir.
func mdFiles(t *testing.T, dir string) []string {
	t.Helper()
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatal(err)
	}
	var files []string
	for _, e := range entries {
		if strings.HasSuffix(e.Name(), ".md") {
			files = append(files, e.Name())
		}
	}
	return files
}

func TestDraft_AutosaveTargetsRecoveryStoreOnly_ThenPromoteBindsSameDocID(t *testing.T) {
	docsDir := t.TempDir()
	m := newDraftWorkspace(t, docsDir)

	if !m.view.IsUntitled() {
		t.Fatal("setup: expected the startup untitled draft displayed")
	}
	draftID := m.view.DocID()
	if draftID == 0 {
		t.Fatal("setup: startup untitled was not upgraded to a durable VFS doc")
	}

	// Type real content into the draft.
	m = focusEditor(m)
	m = typeChar(m, 'h')
	m = typeChar(m, 'i')
	if got := m.editor.Content(); got != "hi" {
		t.Fatalf("setup: editor content = %q, want %q", got, "hi")
	}

	// §1.4.2: the unsaved draft lives in the recovery store ONLY — the
	// journaled content is recoverable, and NO .md file has appeared.
	recovered, err := m.store.RecoverDocument(draftID)
	if err != nil {
		t.Fatalf("RecoverDocument: %v", err)
	}
	if recovered != "hi" {
		t.Fatalf("recovery store content = %q, want %q", recovered, "hi")
	}
	if files := mdFiles(t, docsDir); len(files) != 0 {
		t.Fatalf("autosave must never create a file for a draft; found %v", files)
	}

	// Promote via the REAL rename request the title bar emits on Commit.
	m, cmd := m.Update(title.RenameRequestMsg{Name: "mynote"})
	m = settle(t, m, cmd)

	// The file exists with the draft's bytes, verbatim.
	path := filepath.Join(docsDir, "mynote.md")
	disk, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("promoted file missing: %v", err)
	}
	if string(disk) != "hi" {
		t.Fatalf("promoted file content = %q, want %q", disk, "hi")
	}

	// One atomic bind: same docID, now a file view, and clean.
	if m.view.DocID() != draftID {
		t.Fatalf("promote forked the doc identity: docID %d, want the draft's %d", m.view.DocID(), draftID)
	}
	if !m.view.IsFile() || m.view.Path() != path {
		t.Fatalf("view after promote = (%v, %q), want file view at %q", m.view.Kind(), m.view.Path(), path)
	}
	if m.opentabs.HasDirty() {
		t.Fatal("promote is a save — the doc must be clean afterwards")
	}
}

func TestDraft_DiscardCreatesNoFile(t *testing.T) {
	docsDir := t.TempDir()
	m := newDraftWorkspace(t, docsDir)
	if !m.view.IsUntitled() || m.view.DocID() == 0 {
		t.Fatal("setup: expected a durable startup untitled draft")
	}

	m = focusEditor(m)
	m = typeChar(m, 'x')

	// Close through the REAL guard: ^w on a dirty draft raises the dirty
	// guard; [D]iscard abandons it.
	m, cmd := m.Update(tea.KeyPressMsg{Code: 'w', Mod: tea.ModCtrl})
	m = settle(t, m, cmd)
	if !m.footer.InGuard() {
		t.Fatal("expected the dirty-draft close guard raised by ^w")
	}
	m = pressGuardKey(t, m, 'd')
	if m.footer.InGuard() {
		t.Fatal("guard should be cleared after [D]iscard")
	}

	// §1.4.2: discarding a draft never creates a file.
	if files := mdFiles(t, docsDir); len(files) != 0 {
		t.Fatalf("discarding a draft must not create a file; found %v", files)
	}
}
