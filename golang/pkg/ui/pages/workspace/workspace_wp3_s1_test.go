package workspace

import (
	"os"
	"path/filepath"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/vfs"
)

// TestS1_TitleAndChatEditsNeverSpliceIntoFileContent is the plan's S1
// regression (Part V, WP3 validation): type into file A's editor, the title,
// and the chat; store.Content(fileDocID) must be EXACTLY the editor bytes,
// chat events must exist under chatDocID, and zero events must exist for the
// title anywhere. This test was run against the pre-WP3 tree first and
// FAILED there: store.Content(fileDocID) came back "CTfile content\n"
// instead of "Xfile content\n" — both the chat 'C' and title 'T' keystrokes
// spliced into the file's recovered content ahead of the real edit, because
// journalEdit recorded every surface against m.view.DocID() and
// RecoverDocument has no surface filter (S1, active pre-WP3).
func TestS1_TitleAndChatEditsNeverSpliceIntoFileContent(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "a.md")
	if err := os.WriteFile(path, []byte("file content\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, path, "file content\n")
	fileDocID := m.view.DocID()
	if fileDocID == 0 {
		t.Fatal("setup: expected a real docID for the file")
	}
	chatDocID := m.chatDocID
	if chatDocID == 0 {
		t.Fatal("setup: expected a reserved chat docID")
	}

	// Type into the file's editor.
	m = focusEditor(m)
	m, _ = m.Update(tea.KeyPressMsg{Code: 'X', Text: "X"})
	if got := m.editor.Content(); got != "Xfile content\n" {
		t.Fatalf("setup: editor content = %q", got)
	}

	// Type into the title (rename input).
	m = m.setFocus(paneTitle)
	m.title = m.title.FocusAndSelectAll()
	m, _ = m.Update(tea.KeyPressMsg{Code: 'T', Text: "T"})

	// Type into the chat prompt.
	m = m.setFocus(paneChat)
	m, _ = m.Update(tea.KeyPressMsg{Code: 'C', Text: "C"})

	// The file's recovered content must be EXACTLY the editor bytes — no
	// title/chat keystroke spliced in.
	got, err := m.store.Content(fileDocID)
	if err != nil {
		t.Fatalf("store.Content(fileDocID): %v", err)
	}
	if got != "Xfile content\n" {
		t.Fatalf("S1: store.Content(fileDocID) = %q, want %q — title/chat keystrokes spliced into file content",
			got, "Xfile content\n")
	}

	// Chat events must exist under chatDocID (not the file's docID).
	chatEdits, err := m.store.AllEdits(chatDocID)
	if err != nil {
		t.Fatalf("AllEdits(chatDocID): %v", err)
	}
	if len(chatEdits) == 0 {
		t.Fatal("S1: expected chat keystrokes journaled under chatDocID, found none")
	}
	foundC := false
	for _, batch := range chatEdits {
		for _, e := range batch {
			if e.Insert == "C" {
				foundC = true
			}
		}
	}
	if !foundC {
		t.Fatalf("S1: chat keystroke 'C' not found under chatDocID's journal: %+v", chatEdits)
	}

	// Zero events for the title anywhere (never journaled: ephemeral rename
	// input, finalized on Enter).
	fileEdits, err := m.store.AllEdits(fileDocID)
	if err != nil {
		t.Fatalf("AllEdits(fileDocID): %v", err)
	}
	for _, batch := range fileEdits {
		for _, e := range batch {
			if e.Insert == "T" {
				t.Fatalf("S1: a title keystroke ('T') was journaled under fileDocID: %+v", e)
			}
		}
	}
	for _, batch := range chatEdits {
		for _, e := range batch {
			if e.Insert == "T" {
				t.Fatalf("S1: a title keystroke ('T') was journaled under chatDocID: %+v", e)
			}
		}
	}
}

// TestH4_ChatRedoTruncationDoesNotAffectFile is the plan's WP3
// "redo-truncation" validation: undo in file A, then type in chat — file A's
// redo must still be available (H4: AppendEdit's redo-truncation used to be
// able to delete a DIFFERENT document's future events when title/chat shared
// its docID; with chat on its own reserved document, a chat edit can only
// ever truncate chat's own redo stack).
func TestH4_ChatRedoTruncationDoesNotAffectFile(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "a.md")
	if err := os.WriteFile(path, []byte(""), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, path, "")
	fileDocID := m.view.DocID()

	// Type a word into the file (non-coalesceable across the 300ms window is
	// not needed here — a single undo step is enough).
	m = focusEditor(m)
	m, _ = m.Update(tea.KeyPressMsg{Code: 'A', Text: "A"})
	if got := m.editor.Content(); got != "A" {
		t.Fatalf("setup: editor content = %q", got)
	}

	// Undo it — file A now has a redo target.
	m, _ = m.handleUndo()
	if got := m.editor.Content(); got != "" {
		t.Fatalf("setup: undo did not revert editor: got %q", got)
	}
	if _, ok, err := m.store.RedoPeek(fileDocID); err != nil || !ok {
		t.Fatalf("setup: expected a redo target for fileDocID before the chat edit: ok=%v err=%v", ok, err)
	}

	// Now type in chat.
	m = m.setFocus(paneChat)
	m, _ = m.Update(tea.KeyPressMsg{Code: 'Z', Text: "Z"})

	// File A's redo target must survive — a chat edit can only truncate
	// chat's OWN future, never file A's (I2: separate event streams).
	step, ok, err := m.store.RedoPeek(fileDocID)
	if err != nil {
		t.Fatalf("H4: RedoPeek(fileDocID) after chat edit: %v", err)
	}
	if !ok {
		t.Fatal("H4: file A's redo target was lost after a chat edit — chat truncated the wrong document's future")
	}
	if len(step.Edits) != 1 || step.Edits[0].Insert != "A" {
		t.Fatalf("H4: unexpected redo step for fileDocID: %+v", step)
	}
}
