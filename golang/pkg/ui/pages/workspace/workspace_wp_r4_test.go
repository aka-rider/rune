package workspace

import (
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"os"
	"path/filepath"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/internal/editortest"
	"rune/pkg/docstate"
	"rune/pkg/editor/buffer"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/components/opentabs"
	"rune/pkg/vfs"
)

// TestQuit_AbortsOnDivergedTab_NothingSilentlySkipped is the F7 regression:
// quit "Save all" with a diverged tab must abort the WHOLE quit (never
// silently `continue` past it while saving the REST of the batch) and
// surface the conflict guard. Pre-fix, saveAllDirtyForQuit silently skipped
// the diverged doc and quit anyway with whatever else it could save,
// leaving the user believing "Save all" saved everything.
func TestQuit_AbortsOnDivergedTab_NothingSilentlySkipped(t *testing.T) {
	const ancestor = "initial content"
	const ours = "ours unsaved edits"
	const theirs = "theirs external change"
	m, docA, pathA := setupLoadConflict(t, ancestor, ours, theirs)
	_ = docA

	// Dismiss the load-time guard (Esc) — Sync stays Diverged (mirrors
	// TestB1_EscThenQuitSaveRefused's setup).
	m, _ = m.Update(footer.DataLossGuardResponseMsg{Response: footer.DataLossCancel})
	if m.guard.kind == guardConflict {
		t.Fatal("setup: expected pendingConflict cleared by Esc")
	}

	// A second, independent dirty doc — an ordinary unsaved edit, no conflict.
	dir := filepath.Dir(pathA)
	pathB := filepath.Join(dir, "b.md")
	if err := os.WriteFile(pathB, []byte("b-v1"), 0o644); err != nil {
		t.Fatal(err)
	}
	m, cmd := m.requestOpenPath(0, pathB)
	m = settle(t, m, cmd)
	docB := m.view.DocID()
	if docB == 0 {
		t.Fatal("store not available")
	}
	if _, err := m.store.AppendEdit(docB,
		[]buffer.AppliedEdit{{Start: 0, End: len("b-v1"), Deleted: "b-v1", Insert: "b-v2 unsaved"}}, nil, nil); err != nil {
		t.Fatalf("AppendEdit: %v", err)
	}
	m.editor, _ = m.editor.SetContent("b-v2 unsaved")
	m.opentabs = m.opentabs.SetDirty(opentabs.TabHandle{DocID: docB}, true)

	// Quit "Save all".
	m, cmd = m.saveAllDirtyForQuit()
	m = settle(t, m, cmd)

	// Quit must be ABORTED — no teardown, store still open.
	if m.store == nil {
		t.Fatal("quit must be aborted — store should not have been torn down")
	}
	if !m.footer.InGuard() || m.footer.GuardKind() != footer.GuardMerge {
		t.Fatalf("expected the conflict guard visible after quit-abort; inGuard=%v kind=%v", m.footer.InGuard(), m.footer.GuardKind())
	}
	if m.guard.kind != guardConflict {
		t.Fatal("expected pendingConflict re-raised")
	}

	// B's edit must NOT have been silently saved either — nothing skipped.
	diskB, err := os.ReadFile(pathB)
	if err != nil {
		t.Fatal(err)
	}
	if string(diskB) != "b-v1" {
		t.Fatalf("B must not be silently saved during an aborted quit: disk=%q, want unchanged %q", diskB, "b-v1")
	}
	dirtyB, err := m.store.IsDirty(docB)
	if err != nil {
		t.Fatal(err)
	}
	if !dirtyB {
		t.Fatal("B should remain dirty — quit aborted before saving anything")
	}

	// After resolving A's conflict, quit completes. A REAL keypress (not a
	// direct DataLossGuardResponseMsg injection) — the footer's OWN guard
	// state (guardKind/guardOptions) only clears as a side effect of its
	// OWN key-press handling; injecting the response message directly
	// would leave the footer's guard flag stale even though pendingConflict
	// itself is cleared, which is not what a real user interaction does.
	m, cmd = m.Update(tea.KeyPressMsg{Code: 's', Text: "s"})
	m = settle(t, m, cmd)
	if m.guard.kind == guardConflict {
		t.Fatal("expected A's conflict resolved")
	}
	if m.footer.InGuard() {
		t.Fatal("expected the footer guard cleared after resolving A's conflict")
	}
	m.opentabs = m.opentabs.SetDirty(opentabs.TabHandle{DocID: docB}, true) // B is still independently dirty
	m, cmd = m.saveAllDirtyForQuit()
	m = settle(t, m, cmd)
	if m.footer.InGuard() {
		t.Fatalf("expected quit to complete once the divergence is resolved; still guarded: kind=%v", m.footer.GuardKind())
	}
}

// TestDiskChangedHint_OrdinaryEditDoesNotTrigger is the F10 regression:
// typing an ordinary character (SyncBufferAhead — disk hasn't moved) must
// NEVER trigger the passive "changed on disk" hint; a genuine external
// write must.
func TestDiskChangedHint_OrdinaryEditDoesNotTrigger(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	if err := os.WriteFile(path, []byte("hello"), 0o644); err != nil {
		t.Fatal(err)
	}
	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, path, "hello")
	docID := m.view.DocID()
	if docID == 0 {
		t.Fatal("store not available")
	}

	// Type one char — a REAL journaled edit (BufferAhead: ordinary unsaved
	// edit, disk hasn't moved).
	if _, err := m.store.AppendEdit(docID,
		[]buffer.AppliedEdit{{Start: 5, End: 5, Insert: "!"}}, nil, nil); err != nil {
		t.Fatalf("AppendEdit: %v", err)
	}
	m.editor, _ = m.editor.SetContent("hello!")

	msg1, ok := probeDocCmd(m.store, docID, path)().(probeResultMsg)
	if !ok {
		t.Fatalf("expected probeResultMsg")
	}
	m, _ = m.handleProbeResult(msg1)
	if m.diskChangedHint {
		t.Fatal("F10: an ordinary unsaved edit (BufferAhead) must NOT trigger the changed-on-disk hint")
	}

	// A REAL external write — now genuinely changed.
	if err := os.WriteFile(path, []byte("external change"), 0o644); err != nil {
		t.Fatal(err)
	}
	msg2, ok := probeDocCmd(m.store, docID, path)().(probeResultMsg)
	if !ok {
		t.Fatalf("expected probeResultMsg")
	}
	m, _ = m.handleProbeResult(msg2)
	if !m.diskChangedHint {
		t.Fatal("expected the hint to appear for a genuine external change")
	}
}

// TestConflictDiscard_CorruptBlobRefusesAndSurfaces is the F4 regression:
// a corrupt blob backing theirs must refuse the [D]iscard resolution
// outright (never substitute "" and silently wipe the buffer) and surface
// the error; no resolve observation is committed.
func TestConflictDiscard_CorruptBlobRefusesAndSurfaces(t *testing.T) {
	dir := t.TempDir()
	store, warn, err := docstate.OpenAt(dir)
	if err != nil {
		t.Fatalf("OpenAt: %v", err)
	}
	t.Cleanup(func() { store.Close() })
	if warn != "" {
		t.Fatalf("unexpected degradation warning: %q", warn)
	}

	m := newTestWorkspace(t)
	m = m.WithFS(vfs.Disk{})
	store.UseFS(m.fsys())
	m, cmd := m.Update(StoreReadyMsg{Store: store})
	m = settle(t, m, cmd)

	path := filepath.Join(dir, "note.md")
	const ours = "ours original"
	const theirs = "theirs external change"
	if err := os.WriteFile(path, []byte(ours), 0o644); err != nil {
		t.Fatal(err)
	}
	m = loadFile(m, path, ours)
	docID := m.view.DocID()
	if docID == 0 {
		t.Fatal("store not available")
	}
	m = focusEditor(m)

	if err := os.WriteFile(path, []byte(theirs), 0o644); err != nil {
		t.Fatal(err)
	}
	m, saveCmd := m.startSave()
	if saveCmd == nil {
		t.Fatal("setup: startSave returned nil cmd")
	}
	m, cmd = m.Update(saveCmd())
	m = settle(t, m, cmd)
	if m.guard.kind != guardConflict {
		t.Fatal("setup: expected conflict guard raised")
	}

	// Corrupt the blob row backing theirs' content.
	sum := sha256.Sum256([]byte(theirs))
	theirsHash := hex.EncodeToString(sum[:])
	rawDB, err := sql.Open("sqlite3", filepath.Join(dir, "rune.db"))
	if err != nil {
		t.Fatalf("open raw db: %v", err)
	}
	res, err := rawDB.Exec(`UPDATE blobs SET content=? WHERE hash=?`, []byte("not valid zstd content"), theirsHash)
	if err != nil {
		t.Fatalf("corrupt blob: %v", err)
	}
	if n, _ := res.RowsAffected(); n != 1 {
		t.Fatalf("corrupt blob: expected 1 row affected for hash %s, got %d", theirsHash, n)
	}
	if err := rawDB.Close(); err != nil {
		t.Fatalf("close raw db: %v", err)
	}
	if _, getErr := m.store.GetBlob(theirsHash); getErr == nil {
		t.Fatalf("setup: GetBlob(theirsHash) succeeded despite corruption — hash mismatch in test setup")
	}

	preContent := m.editor.Content()
	preSaved, _, err := m.store.SavedObs(docID)
	if err != nil {
		t.Fatal(err)
	}

	// Press [D]iscard — a REAL key, routed through the active guard. Drains
	// only until the error appears in the footer (NOT a full settle): the
	// error's own real-time auto-dismiss timer is itself a further Cmd this
	// async chain would otherwise recurse into and execute synchronously,
	// clearing the message again before this assertion ever sees it —
	// exactly the stop-before-next-Cmd contract editortest.DrainUntil
	// implements. The marker is "resolve" (the error's own fixed prefix,
	// "resolve %q: ..."), not the tempdir-rooted path itself — the footer
	// truncates at its render width, and a t.TempDir() path is long enough
	// to truncate before its own basename ever appears.
	m, cmd = m.Update(tea.KeyPressMsg{Code: 'd', Text: "d"})
	m = editortest.DrainUntil(m, cmd, Model.Update, func(m Model, _ tea.Msg) bool {
		return strings.Contains(m.footer.View(), "resolve")
	})

	if m.editor.Content() != preContent {
		t.Fatalf("buffer changed despite corrupt blob: got %q, want unchanged %q", m.editor.Content(), preContent)
	}
	postSaved, _, err := m.store.SavedObs(docID)
	if err != nil {
		t.Fatal(err)
	}
	if postSaved.ID != preSaved.ID {
		t.Fatalf("a resolve was committed despite corrupt blob: saved_obs changed %d -> %d", preSaved.ID, postSaved.ID)
	}
	body := m.footer.View()
	if !strings.Contains(body, "resolve") {
		t.Fatalf("expected the corrupt-blob error surfaced in the footer, got: %s", body)
	}
}

// TestDegradedStore_PersistentBannerAndSaveConfirms is the WP-R4 item 5
// regression: a degraded store (in-memory fallback) must show a PERSISTENT
// banner and gate ⌘S behind an explicit confirmation before writing —
// capture-into-RAM must never masquerade as durability.
func TestDegradedStore_PersistentBannerAndSaveConfirms(t *testing.T) {
	noPermDir := t.TempDir()
	if err := os.Chmod(noPermDir, 0o000); err != nil {
		t.Skip("cannot chmod temp dir:", err)
	}
	t.Cleanup(func() { os.Chmod(noPermDir, 0o700) })

	store, warn, err := docstate.Open(noPermDir)
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	t.Cleanup(func() { store.Close() })
	if !store.Degraded() {
		t.Fatal("setup: expected a degraded store")
	}

	workDir := t.TempDir()
	m := newTestWorkspace(t)
	m = m.WithFS(vfs.Disk{})
	store.UseFS(m.fsys())
	m, cmd := m.Update(StoreReadyMsg{Store: store, Warning: warn})
	m = settle(t, m, cmd)

	if !strings.Contains(m.footer.View(), "Storage degraded") {
		t.Fatal("expected the persistent degraded banner set")
	}

	path := filepath.Join(workDir, "note.md")
	if err := os.WriteFile(path, []byte("hello"), 0o644); err != nil {
		t.Fatal(err)
	}
	m = loadFile(m, path, "hello")
	if m.view.DocID() == 0 {
		t.Fatal("docID unavailable")
	}
	m.editor, _ = m.editor.SetContent("hello!")

	m, _ = m.startSave()
	if m.activeSave.InFlight {
		t.Fatal("⌘S must NOT proceed to an in-flight save while degraded and unconfirmed")
	}
	if !m.footer.InGuard() || m.footer.GuardKind() != footer.GuardDegraded {
		t.Fatalf("expected GuardDegraded raised before writing; inGuard=%v kind=%v", m.footer.InGuard(), m.footer.GuardKind())
	}
	diskBefore, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if string(diskBefore) != "hello" {
		t.Fatalf("disk written before confirmation: %q", diskBefore)
	}

	// Confirm — the save now proceeds.
	m, cmd = m.Update(tea.KeyPressMsg{Code: 'y', Text: "y"})
	m = settle(t, m, cmd)
	diskAfter, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if string(diskAfter) != "hello!" {
		t.Fatalf("disk after confirming degraded save = %q, want %q", diskAfter, "hello!")
	}
}

// TestHardlinkedFile_WarnsOnOpen is the WP-R4 item 6 regression: opening a
// file with nlink > 1 must surface a footer warning that saving breaks the
// link.
func TestHardlinkedFile_WarnsOnOpen(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	linkPath := filepath.Join(dir, "note-link.md")
	if err := os.WriteFile(path, []byte("hello"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Link(path, linkPath); err != nil {
		t.Skip("hardlinks not supported on this filesystem:", err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, path, "hello")
	if m.view.DocID() == 0 {
		t.Fatal("store not available")
	}

	body := m.footer.View()
	if !strings.Contains(body, "hardlinked") {
		t.Fatalf("expected a hardlink warning in the footer, got: %s", body)
	}
}
