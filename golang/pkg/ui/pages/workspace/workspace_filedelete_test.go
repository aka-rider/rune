package workspace

import (
	"errors"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/components/filetree"
	"rune/pkg/ui/components/footer"
)

// ─────────────────────────────────────────────────────────────────────────────
// Delete in explorer (trash)
// ─────────────────────────────────────────────────────────────────────────────

// TestFileDeletedMsg_ClosesActiveTab verifies that FileDeletedMsg for the active
// document closes its tab and transitions the editor away from the deleted path.
func TestFileDeletedMsg_ClosesActiveTab(t *testing.T) {
	m := newTestWorkspace(t)
	m = loadFile(m, "a.md", "content A")
	if m.view.Path() != "a.md" {
		t.Fatalf("prerequisite: active path = %q, want a.md", m.view.Path())
	}

	m, cmd := m.Update(FileDeletedMsg{Path: "a.md"})
	// Drive any resulting Cmds (executeClose may issue a CreateUntitled cmd).
	for _, msg := range execCmds(cmd) {
		m, _ = m.Update(msg)
	}

	if m.view.Path() == "a.md" {
		t.Fatal("active view path is still a.md after FileDeletedMsg — tab not closed")
	}
}

// TestFileDeletedMsg_RemovesBackgroundTab verifies that FileDeletedMsg for a
// background tab removes that tab without disturbing the active view.
func TestFileDeletedMsg_RemovesBackgroundTab(t *testing.T) {
	m := newTestWorkspace(t)
	m = loadFile(m, "a.md", "content A")
	m = loadFile(m, "b.md", "content B") // second tab; b.md is now active

	if m.view.Path() != "b.md" {
		t.Fatalf("prerequisite: active path = %q, want b.md", m.view.Path())
	}
	tabsBefore := m.opentabs.Len()

	// a.md is the background tab — deleting it must not change the active view.
	m, _ = m.Update(FileDeletedMsg{Path: "a.md"})

	if m.view.Path() != "b.md" {
		t.Fatalf("active view changed to %q after background-tab delete — should stay b.md", m.view.Path())
	}
	if m.opentabs.Len() != tabsBefore-1 {
		t.Fatalf("opentabs.Len() = %d, want %d (background tab not removed)", m.opentabs.Len(), tabsBefore-1)
	}
}

// TestFileDeleteErrorMsg_ShowsFooterError verifies that FileDeleteErrorMsg
// surfaces the error in the footer and does not close any tab.
func TestFileDeleteErrorMsg_ShowsFooterError(t *testing.T) {
	m := newTestWorkspace(t)
	m = loadFile(m, "a.md", "content A")
	pathBefore := m.view.Path()

	m, _ = m.Update(FileDeleteErrorMsg{
		Path: "a.md",
		Err:  errors.New(`trash "a.md": exit status 1`),
	})

	if m.view.Path() != pathBefore {
		t.Fatalf("active view changed to %q — should not happen on error", m.view.Path())
	}
	footerView := m.footer.View()
	if !strings.Contains(footerView, `trash "a.md"`) {
		t.Fatalf("footer does not show error: %q", footerView)
	}
}

// TestFileDeleteRequestedMsg_RaisesGuard verifies that FileDeleteRequestedMsg
// for a clean (non-dirty-active) file raises the trash confirmation guard without
// touching the filetree entry yet (guard-first, §1.4.4).
func TestFileDeleteRequestedMsg_RaisesGuard(t *testing.T) {
	m := newTestWorkspace(t)
	m = loadFile(m, "a.md", "content A")

	m, _ = m.Update(filetree.DirLoadedMsg{
		Root: ".",
		Entries: []filetree.Entry{
			{Name: "b.md", Path: "b.md"},
		},
	})
	entriesBefore := m.filetree.Len()

	m, _ = m.Update(filetree.FileDeleteRequestedMsg{Path: "b.md"})

	if m.filetree.Len() != entriesBefore {
		t.Fatalf("filetree.Len() = %d, want %d — must not remove entry before guard confirmation",
			m.filetree.Len(), entriesBefore)
	}
	if !strings.Contains(m.footer.View(), "Trash file?") {
		t.Fatalf("footer does not show trash guard: %q", m.footer.View())
	}
}

// TestFileDeleteGuardConfirm_RemovesEntryAndIssuesCmd verifies that
// DataLossGuardResponseMsg{DataLossTrash} triggers optimistic removal and a
// fileTrashCmd (§5.4).
func TestFileDeleteGuardConfirm_RemovesEntryAndIssuesCmd(t *testing.T) {
	m := newTestWorkspace(t)
	m = loadFile(m, "a.md", "content A")

	m, _ = m.Update(filetree.DirLoadedMsg{
		Root: ".",
		Entries: []filetree.Entry{
			{Name: "b.md", Path: "b.md"},
		},
	})
	m, _ = m.Update(filetree.FileDeleteRequestedMsg{Path: "b.md"})

	entriesAfterGuard := m.filetree.Len()
	m, cmd := m.Update(footer.DataLossGuardResponseMsg{Response: footer.DataLossTrash})

	if m.filetree.Len() != entriesAfterGuard-1 {
		t.Fatalf("filetree.Len() = %d, want %d — entry not removed after guard confirm",
			m.filetree.Len(), entriesAfterGuard-1)
	}
	if cmd == nil {
		t.Fatal("expected a fileTrashCmd to be issued, got nil Cmd")
	}
}

// TestFileDeleteGuardCancel_LeavesFiletreeUnchanged verifies that
// DataLossGuardResponseMsg{DataLossCancel} leaves the filetree intact.
func TestFileDeleteGuardCancel_LeavesFiletreeUnchanged(t *testing.T) {
	m := newTestWorkspace(t)
	m = loadFile(m, "a.md", "content A")

	m, _ = m.Update(filetree.DirLoadedMsg{
		Root: ".",
		Entries: []filetree.Entry{
			{Name: "b.md", Path: "b.md"},
		},
	})
	m, _ = m.Update(filetree.FileDeleteRequestedMsg{Path: "b.md"})

	entriesAfterGuard := m.filetree.Len()
	m, cmd := m.Update(footer.DataLossGuardResponseMsg{Response: footer.DataLossCancel})

	if m.filetree.Len() != entriesAfterGuard {
		t.Fatalf("filetree.Len() = %d, want %d — cancel must not change filetree",
			m.filetree.Len(), entriesAfterGuard)
	}
	if strings.Contains(m.footer.View(), "Trash file?") {
		t.Fatal("guard still visible after cancel")
	}
	_ = cmd // cancel produces no trash cmd; batch may contain footer state Cmd
}

// TestFileDeleteRequestedMsg_DirtyActiveDoc_BlocksTrash verifies that sending
// FileDeleteRequestedMsg for the currently active dirty document does NOT
// optimistically remove the entry from the filetree (§1.4.4).
func TestFileDeleteRequestedMsg_DirtyActiveDoc_BlocksTrash(t *testing.T) {
	m := newTestWorkspace(t)
	m = withStore(t, m)
	m = loadFile(m, "a.md", "content A")
	m = focusEditor(m)

	// Type to make the buffer dirty.
	m, _ = m.Update(tea.KeyPressMsg{Code: 'x'})
	if !m.opentabs.HasDirty() {
		t.Fatal("prerequisite: buffer must be dirty before testing the guard")
	}

	// Populate filetree so there is an entry to check.
	m, _ = m.Update(filetree.DirLoadedMsg{
		Root: ".",
		Entries: []filetree.Entry{
			{Name: "a.md", Path: "a.md"},
		},
	})
	entriesBefore := m.filetree.Len()

	m, _ = m.Update(filetree.FileDeleteRequestedMsg{Path: "a.md"})

	if m.filetree.Len() != entriesBefore {
		t.Fatalf("filetree.Len() = %d, want %d — dirty guard did not block optimistic remove",
			m.filetree.Len(), entriesBefore)
	}
	footerView := m.footer.View()
	if !strings.Contains(footerView, "Unsaved changes") {
		t.Fatalf("footer does not show unsaved-changes message: %q", footerView)
	}
}
