package workspace

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/docstate"
	"rune/pkg/ui/components/filetree"
	"rune/pkg/ui/components/footer"
	"rune/pkg/vfs"
)

// ─────────────────────────────────────────────────────────────────────────────
// FileSavedMsg with matching RequestID clears InFlight
// ─────────────────────────────────────────────────────────────────────────────

func TestFileSaved_MatchingIDClearsInFlight(t *testing.T) {
	m := withStore(t, newTestWorkspace(t))
	m = loadFile(m, "a.txt", "content A")

	var saveCmd tea.Cmd
	m, saveCmd = m.startSave()
	reqID := m.activeSave.RequestID
	_ = saveCmd

	if !m.activeSave.InFlight {
		t.Fatal("expected InFlight=true")
	}

	m, _ = m.Update(FileSavedMsg{
		Path:      "a.txt",
		RequestID: reqID,
	})

	if m.activeSave.InFlight {
		t.Fatal("expected InFlight=false after matching save ack")
	}
}

func TestFileSaved_NonMatchingIDIgnored(t *testing.T) {
	m := withStore(t, newTestWorkspace(t))
	m = loadFile(m, "a.txt", "content A")

	var saveCmd tea.Cmd
	m, saveCmd = m.startSave()
	_ = saveCmd

	if !m.activeSave.InFlight {
		t.Fatal("expected InFlight=true")
	}

	m, _ = m.Update(FileSavedMsg{
		Path:      "a.txt",
		RequestID: "wrong-id",
	})

	if !m.activeSave.InFlight {
		t.Fatal("expected InFlight still true after non-matching save ack")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// StoreReadyMsg sets the store and warns on degradation
// ─────────────────────────────────────────────────────────────────────────────

// TestRecoverDocument_CorrectAfterStoreReadyRace pins a bug found by first
// ever running FuzzHumanSession (it was wired into the Makefile but had never
// been executed): a file opened via -w before the docstate store is ready
// never gets its durable source='disk' baseline snapshot recorded, because
// handleStoreReadyMsg resolved identity but skipped stampLoadBaseline. A
// LATER stampLoadBaseline call (re-opening this file after switching away)
// then finds no existing disk baseline, blindly stamps whatever is then on
// disk as the anchor — and because that wrongly-late snapshot shares the same
// seq as the correct journaled snapshot, it wins RecoverDocument's id-DESC
// tie-break, silently reconstructing stale/external disk bytes instead of the
// user's journaled edit. This is unrelated to the atomic-rewrite/inode
// investigation; it surfaced purely from exercising this fuzz target for the
// first time.
func TestRecoverDocument_CorrectAfterStoreReadyRace(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "a.md")
	if err := os.WriteFile(path, []byte("original"), 0o644); err != nil {
		t.Fatal(err)
	}

	m := newWorkspaceWithFiles(t, path).WithFS(vfs.Disk{}) // path is a real t.TempDir() file
	// Race: the startup load's FileLoadedMsg arrives BEFORE the store is ready.
	m, _ = m.Update(FileLoadedMsg{Path: path, Result: docstate.LoadResult{DiskContent: "original", Recovered: "original"}, Gen: m.loadGen})
	if m.view.DocID() != 0 {
		t.Fatal("setup: expected docID 0 before the store is wired")
	}

	m = withStore(t, m) // store becomes ready — resolves identity asynchronously (lateBindLoadCmd)
	docID := m.view.DocID()
	if docID == 0 {
		t.Fatal("expected identity resolved once the store is ready")
	}

	// Edit, unsaved.
	m = focusEditor(m)
	m, _ = m.Update(tea.KeyPressMsg{Code: 'x', Text: "x"})
	if got := m.editor.Content(); got != "xoriginal" {
		t.Fatalf("setup: editor content = %q, want %q", got, "xoriginal")
	}

	// Switch away without saving.
	m, cmd := m.requestOpenPath(0, "")
	m = drainCmd(m, cmd)

	// The file changes externally while backgrounded.
	if err := os.WriteFile(path, []byte("external-change"), 0o644); err != nil {
		t.Fatal(err)
	}

	// Switch back.
	m, cmd = m.requestOpenPath(docID, path)
	m = drainCmd(m, cmd)

	got, err := m.store.RecoverDocument(docID)
	if err != nil {
		t.Fatalf("RecoverDocument: %v", err)
	}
	if got != "xoriginal" {
		t.Fatalf("RecoverDocument = %q, want the journaled edit preserved (%q), not stale/external disk bytes", got, "xoriginal")
	}
	// The 3-way conflict (unsaved edit AND external change) must also be
	// surfaced, not silently swallowed — this depends on the same fix.
	if m.footer.GuardKind() != footer.GuardMerge {
		t.Fatalf("expected GuardMerge for a genuine 3-way conflict, got guard kind %v", m.footer.GuardKind())
	}
}

// TestExternalRename_ShowsFooterNotice pins §1.4.6 "tell the user on a
// detected rename": when store.OpenPath resolves a freshly-loaded path to an
// ALREADY-KNOWN docID via matching inode (a genuine external rename — not our
// own atomic-save-induced inode churn, which is routed through Bind and never
// reaches this path), the footer must show a rename notice, not silently
// relabel the tab with no user-visible signal.
func TestExternalRename_ShowsFooterNotice(t *testing.T) {
	dir := t.TempDir()
	oldPath := filepath.Join(dir, "old.md")
	newPath := filepath.Join(dir, "new.md")
	if err := os.WriteFile(oldPath, []byte("hello"), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, oldPath, "hello")
	docID := m.view.DocID()
	if docID == 0 {
		t.Fatal("expected a real docID")
	}

	// External rename: same inode, new path (os.Rename preserves inode).
	if err := os.Rename(oldPath, newPath); err != nil {
		t.Fatal(err)
	}

	// Simulate discovering the renamed file at its new path (e.g. after a
	// directory reload) — a fresh load resolves the SAME docID via inode,
	// with a non-empty RenamedFrom.
	m = loadFile(m, newPath, "hello")

	if m.view.DocID() != docID {
		t.Fatalf("expected the SAME docID after rename detection, got %d want %d", m.view.DocID(), docID)
	}
	if !strings.Contains(m.footer.View(), "Renamed") {
		t.Fatalf("expected a rename notice in the footer, got View=%q", m.footer.View())
	}
}

func TestStoreReadyMsg_SetsStore(t *testing.T) {
	m := newTestWorkspace(t)
	if m.store != nil {
		t.Fatal("expected store nil before StoreReadyMsg")
	}
	// We can't easily open a real store in a unit test without cgo/sqlite;
	// just verify the nil-store path is safe.
	m, _ = m.Update(StoreReadyMsg{Store: nil, Warning: ""})
	// No panic, store stays nil.
	_ = m
}

// ─────────────────────────────────────────────────────────────────────────────
// Filetree reload after dir changes and internal file mutations (Fix A / B1 / B2)
// ─────────────────────────────────────────────────────────────────────────────

// TestDirChangedMsg_EmitsReloadAndKeepsWatchedDir verifies Fix A: dirChangedMsg
// emits a non-nil Cmd (containing the reloadDirCmd + a watcher restart) and
// leaves m.watchedDir set so subsequent changes are still tracked.
// The goroutine restart itself cannot be verified without real FS events; this
// test covers the observable model-level invariants.
func TestDirChangedMsg_EmitsReloadAndKeepsWatchedDir(t *testing.T) {
	m := newTestWorkspace(t)
	m, _ = m.Update(filetree.DirLoadedMsg{Root: "/test", Entries: nil})
	if m.watchedDir != "/test" {
		t.Fatalf("prerequisite: watchedDir = %q, want /test", m.watchedDir)
	}

	m2, cmd := m.Update(dirChangedMsg{})
	if cmd == nil {
		t.Fatal("dirChangedMsg must return a non-nil Cmd (at least reloadDirCmd)")
	}
	if m2.watchedDir != "/test" {
		t.Fatalf("watchedDir after dirChangedMsg = %q, want /test", m2.watchedDir)
	}
}

// TestFileSavedMsg_BindNew_RefreshesFiletree verifies Fix B1: after a ^n file
// creation (FileSavedMsg{BindNew: true}), executing the returned Cmd produces a
// DirReloadedMsg that includes the new file. Without Fix B1 the Cmd is nil and
// the filetree never learns about the new entry.
func TestFileSavedMsg_BindNew_RefreshesFiletree(t *testing.T) {
	fsys := vfs.NewMem()
	if err := fsys.WriteFile("/test/new.md", []byte("hello"), 0o644); err != nil {
		t.Fatal(err)
	}

	m := newTestWorkspace(t)
	m = m.WithFS(fsys)
	// Populate the filetree root so Root() returns "/test".
	m, _ = m.Update(filetree.DirLoadedMsg{Root: "/test", Entries: nil})

	// Simulate a BindNew save: mark an in-flight save, then acknowledge it.
	reqID := "test-bind-new"
	m.activeSave = SaveIdentity{RequestID: reqID, InFlight: true}

	_, cmd := m.Update(FileSavedMsg{
		Path:      "/test/new.md",
		RequestID: reqID,
		BindNew:   true,
	})

	// execCmds is safe here: the BindNew handler only adds reloadDirCmd (no
	// startWatch), and reloadDirCmd on a vfs.Mem returns immediately.
	var gotReload bool
	for _, msg := range execCmds(cmd) {
		rld, ok := msg.(filetree.DirReloadedMsg)
		if !ok {
			continue
		}
		gotReload = true
		for _, e := range rld.Entries {
			if e.Name == "new.md" {
				return // new file present in the reload — fix is working
			}
		}
		t.Fatalf("DirReloadedMsg.Entries = %v, want entry named new.md", rld.Entries)
	}
	if !gotReload {
		t.Fatal("FileSavedMsg{BindNew:true} produced no DirReloadedMsg — filetree will never show the new file")
	}
}

// TestFileRenamedMsg_RefreshesFiletree verifies Fix B2: after a file rename,
// executing the returned Cmd produces a DirReloadedMsg with the new name.
// Without Fix B2 no reload is emitted and the filetree keeps the old name.
func TestFileRenamedMsg_RefreshesFiletree(t *testing.T) {
	fsys := vfs.NewMem()
	// Only the renamed-to path needs to exist for reloadDirCmd.
	if err := fsys.WriteFile("/test/new.md", []byte("hello"), 0o644); err != nil {
		t.Fatal(err)
	}

	m := newTestWorkspace(t)
	m = m.WithFS(fsys)
	m, _ = m.Update(filetree.DirLoadedMsg{
		Root:    "/test",
		Entries: []filetree.Entry{{Name: "old.md", Path: "/test/old.md"}},
	})
	m = loadFile(m, "/test/old.md", "hello")

	_, cmd := m.Update(FileRenamedMsg{OldPath: "/test/old.md", NewPath: "/test/new.md"})

	// execCmds is safe: FileRenamedMsg handler adds only reloadDirCmd (no startWatch).
	var gotReload bool
	for _, msg := range execCmds(cmd) {
		rld, ok := msg.(filetree.DirReloadedMsg)
		if !ok {
			continue
		}
		gotReload = true
		for _, e := range rld.Entries {
			if e.Name == "new.md" {
				return // renamed file present — fix is working
			}
		}
		t.Fatalf("DirReloadedMsg.Entries = %v, want entry named new.md", rld.Entries)
	}
	if !gotReload {
		t.Fatal("FileRenamedMsg produced no DirReloadedMsg — filetree will never show the renamed file")
	}
}
