package workspace

// Tests for the GuardDeleted footer guard (§ACTIVE(2) — file-deleted-on-disk
// guard + focus-trap fix). Every test drives the real m.Update cycle with a
// realistic setup (a docstate store, a real load through the load-generation
// handshake, and journaled keystrokes) rather than calling handlers directly —
// prior handler-only tests in this feature area gave false confidence.

import (
	"os"
	"path/filepath"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/docstate"
	"rune/pkg/ui/components/filetree"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/components/opentabs"
	"rune/pkg/vfs"
)

// deletedGuardFixture loads path (real disk, via the load-generation
// handshake), focuses the editor, and journals one edit so the doc is dirty —
// the realistic starting point for every GuardDeleted test.
func deletedGuardFixture(t *testing.T, path string) Model {
	t.Helper()
	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = openReal(m, path)
	if m.view.Path() != path {
		t.Fatalf("setup: load failed, view.Path() = %q, want %q", m.view.Path(), path)
	}
	m = focusEditor(m)
	m = typeChar(m, 'X')
	if !m.opentabs.HasDirty() {
		t.Fatal("setup: expected dirty after edit")
	}
	return m
}

// settle recursively executes cmd and every message it (and its children)
// produce, feeding each back into m.Update — draining an async round-trip
// (e.g. keypress → footer response msg → materializeCmd → FileSavedMsg)
// exactly as the real Bubble Tea runtime would.
func settle(t *testing.T, m Model, cmd tea.Cmd) Model {
	t.Helper()
	for _, msg := range execCmds(cmd) {
		var next tea.Cmd
		m, next = m.Update(msg)
		m = settle(t, m, next)
	}
	return m
}

// pressGuardKey sends a real tea.KeyPressMsg for ch through the full Update
// cycle (handleKeyPress → footer.Update, exactly like the real key routing —
// see workspace_update_keys.go Priority 2.1) and drains every resulting
// message, so the footer's own guard-clearing runs exactly as it does in
// production (tests that inject footer.DataLossGuardResponseMsg directly skip
// that step and see a stale InGuard()).
func pressGuardKey(t *testing.T, m Model, ch rune) Model {
	t.Helper()
	m, cmd := m.Update(tea.KeyPressMsg{Code: ch, Text: string(ch)})
	return settle(t, m, cmd)
}

// pressEsc sends the real Escape key through the full Update cycle.
func pressEsc(t *testing.T, m Model) Model {
	t.Helper()
	m, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyEscape})
	return settle(t, m, cmd)
}

// ─────────────────────────────────────────────────────────────────────────────
// Detection — dirChangedMsg raises the guard (RELOAD-NOMUT holds)
// ─────────────────────────────────────────────────────────────────────────────

func TestDeletedGuard_DirChangedRaisesGuard(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	if err := os.WriteFile(path, []byte("hello"), 0o644); err != nil {
		t.Fatal(err)
	}

	m := deletedGuardFixture(t, path)
	wantContent := m.editor.Content()
	wantDirty := m.opentabs.HasDirty()

	if err := os.Remove(path); err != nil {
		t.Fatal(err)
	}

	m, dcCmd := m.Update(dirChangedMsg{})
	m = drainCmd(m, dcCmd)

	if !m.footer.InGuard() || m.footer.GuardKind() != footer.GuardDeleted {
		t.Fatalf("GuardDeleted not raised: InGuard=%v kind=%v", m.footer.InGuard(), m.footer.GuardKind())
	}
	if !m.pendingDeleted.active {
		t.Fatal("pendingDeleted.active not set")
	}
	// RELOAD-NOMUT (internal/fuzz/ui/workspace/workspace.go:171-186): a
	// dirChangedMsg must never mutate buffer content or dirty status.
	if m.editor.Content() != wantContent {
		t.Fatalf("buffer mutated by dirChangedMsg: %q want %q", m.editor.Content(), wantContent)
	}
	if m.opentabs.HasDirty() != wantDirty {
		t.Fatalf("dirty mutated by dirChangedMsg: %v want %v", m.opentabs.HasDirty(), wantDirty)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// [S]ave — recreates the file from the live buffer
// ─────────────────────────────────────────────────────────────────────────────

func TestDeletedGuard_SaveRecreatesFile(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	if err := os.WriteFile(path, []byte("hello"), 0o644); err != nil {
		t.Fatal(err)
	}

	m := deletedGuardFixture(t, path)
	wantContent := m.editor.Content()

	if err := os.Remove(path); err != nil {
		t.Fatal(err)
	}
	m, dcCmd := m.Update(dirChangedMsg{})
	m = drainCmd(m, dcCmd)
	if !m.pendingDeleted.active {
		t.Fatal("prerequisite: guard not raised")
	}

	m = pressGuardKey(t, m, 's')

	got, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("file not recreated: %v", err)
	}
	if string(got) != wantContent {
		t.Fatalf("recreated content = %q, want %q", got, wantContent)
	}
	if m.opentabs.HasDirty() {
		t.Fatal("tab should be clean after recreate-save")
	}
	if m.footer.InGuard() {
		t.Fatal("guard should be cleared after save")
	}
	if m.pendingDeleted.active {
		t.Fatal("pendingDeleted should be cleared after save")
	}
}

// TestDeletedGuard_SaveRecreatesParentDir covers the rm -r case: the parent
// directory was removed along with the file, so materializeCmd's plain
// WriteFile would fail with ENOENT unless handleDeletedSave's MkdirAll ran
// first.
func TestDeletedGuard_SaveRecreatesParentDir(t *testing.T) {
	dir := t.TempDir()
	sub := filepath.Join(dir, "sub")
	if err := os.MkdirAll(sub, 0o755); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(sub, "note.md")
	if err := os.WriteFile(path, []byte("hello"), 0o644); err != nil {
		t.Fatal(err)
	}

	m := deletedGuardFixture(t, path)
	wantContent := m.editor.Content()

	if err := os.RemoveAll(sub); err != nil {
		t.Fatal(err)
	}
	m, dcCmd := m.Update(dirChangedMsg{})
	m = drainCmd(m, dcCmd)
	if !m.pendingDeleted.active {
		t.Fatal("prerequisite: guard not raised")
	}

	m = pressGuardKey(t, m, 's')

	got, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("file/parent dir not recreated: %v", err)
	}
	if string(got) != wantContent {
		t.Fatalf("recreated content = %q, want %q", got, wantContent)
	}
}

// TestDeletedGuard_SaveMkdirFailureReArmsGuard: when the parent directory
// cannot be recreated (here: a regular file now occupies that path, so
// MkdirAll fails with ENOTDIR — a portable stand-in for a permission error),
// handleDeletedSave must restore pendingDeleted AND re-arm the visible
// GuardDeleted prompt so the user can retry, rather than leaving an invisible
// pendingDeleted with no way to act on it.
func TestDeletedGuard_SaveMkdirFailureReArmsGuard(t *testing.T) {
	dir := t.TempDir()
	sub := filepath.Join(dir, "sub")
	if err := os.MkdirAll(sub, 0o755); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(sub, "note.md")
	if err := os.WriteFile(path, []byte("hello"), 0o644); err != nil {
		t.Fatal(err)
	}

	m := deletedGuardFixture(t, path)

	// Blow away the parent dir entirely (a clean ENOENT — currentFileMissing
	// must detect this the normal way) and confirm detection first.
	if err := os.RemoveAll(sub); err != nil {
		t.Fatal(err)
	}
	m, dcCmd := m.Update(dirChangedMsg{})
	m = drainCmd(m, dcCmd)
	if !m.pendingDeleted.active {
		t.Fatal("prerequisite: guard not raised")
	}

	// THEN occupy the parent's path with a regular FILE so the upcoming
	// MkdirAll(sub) inside handleDeletedSave is guaranteed to fail (ENOTDIR) —
	// a portable stand-in for "the parent dir cannot be recreated" (e.g. a
	// permission error) that doesn't depend on running as non-root.
	if err := os.WriteFile(sub, []byte("blocker"), 0o644); err != nil {
		t.Fatal(err)
	}

	// Drive the 's' keypress through the footer (clears its own guard state,
	// exactly like production), then dispatch the resulting response message —
	// but do NOT execute the follow-on error-banner dismiss-timer Cmd (a real
	// 5s sleep in this component); we only need the state right after
	// handleDeletedSave's mkdir-failure branch runs.
	m, cmd := m.Update(tea.KeyPressMsg{Code: 's', Text: "s"})
	msgs := execCmds(cmd)
	if len(msgs) != 1 {
		t.Fatalf("expected exactly one message from the 's' keypress, got %d: %v", len(msgs), msgs)
	}
	m, _ = m.Update(msgs[0])

	if !m.pendingDeleted.active {
		t.Fatal("mkdir failure must restore pendingDeleted so the user can retry")
	}
	if !m.footer.InGuard() || m.footer.GuardKind() != footer.GuardDeleted {
		t.Fatalf("mkdir failure must re-arm GuardDeleted so [S]/[D] stay retryable; InGuard=%v kind=%v",
			m.footer.InGuard(), m.footer.GuardKind())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// [D]iscard — purges VFS history and closes the tab
// ─────────────────────────────────────────────────────────────────────────────

func TestDeletedGuard_DiscardPurgesDocAndClosesTab(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	if err := os.WriteFile(path, []byte("hello"), 0o644); err != nil {
		t.Fatal(err)
	}

	m := deletedGuardFixture(t, path)
	docID := m.view.DocID()
	if docID == 0 {
		t.Fatal("prerequisite: docID not resolved (store not wired)")
	}

	if err := os.Remove(path); err != nil {
		t.Fatal(err)
	}
	m, dcCmd := m.Update(dirChangedMsg{})
	m = drainCmd(m, dcCmd)
	if !m.pendingDeleted.active {
		t.Fatal("prerequisite: guard not raised")
	}

	m = pressGuardKey(t, m, 'd')

	if m.view.Path() == path {
		t.Fatal("tab was not closed — still viewing the discarded doc")
	}
	if has, err := m.store.HasHistory(docID); err == nil && has {
		t.Fatal("doc history was not purged — DeleteDoc did not run")
	}
	if m.footer.InGuard() {
		t.Fatal("guard should be cleared after discard")
	}
	if m.pendingDeleted.active {
		t.Fatal("pendingDeleted should be cleared after discard")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Esc — never forces discard; a later ⌘S recreates the file
// ─────────────────────────────────────────────────────────────────────────────

func TestDeletedGuard_EscThenNormalSaveReRaisesGuard(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	if err := os.WriteFile(path, []byte("hello"), 0o644); err != nil {
		t.Fatal(err)
	}

	m := deletedGuardFixture(t, path)
	wantContent := m.editor.Content()

	if err := os.Remove(path); err != nil {
		t.Fatal(err)
	}
	m, dcCmd := m.Update(dirChangedMsg{})
	m = drainCmd(m, dcCmd)
	if !m.pendingDeleted.active {
		t.Fatal("prerequisite: guard not raised")
	}

	m = pressEsc(t, m)

	if m.editor.Content() != wantContent {
		t.Fatal("Esc must not mutate the buffer")
	}
	if m.footer.InGuard() {
		t.Fatal("guard must be cleared on Esc")
	}
	if m.pendingDeleted.active {
		t.Fatal("pendingDeleted must be cleared on Esc")
	}
	if !m.opentabs.HasDirty() {
		t.Fatal("doc should still be dirty after Esc — nothing was saved or discarded")
	}

	// A subsequent normal ⌘S must NOT silently recreate the file: materializeCmd's
	// targetVanished refuses the write, and the FileSaveErrorMsg re-raises
	// GuardDeleted (save-time detection unified with the idle dirChangedMsg path).
	// The user must explicitly choose [S] again.
	m, cmd := m.startSave()
	m = settle(t, m, cmd)
	if !m.footer.InGuard() || m.footer.GuardKind() != footer.GuardDeleted {
		t.Fatalf("normal ⌘S on a vanished file must re-raise GuardDeleted, got InGuard=%v kind=%v", m.footer.InGuard(), m.footer.GuardKind())
	}
	if !m.pendingDeleted.active {
		t.Fatal("pendingDeleted must be re-armed by the save-time targetVanished detection")
	}
	if _, err := os.Stat(path); !os.IsNotExist(err) {
		t.Fatal("normal ⌘S must not have recreated the file (targetVanished refuses)")
	}

	// Pressing [S] on the re-raised guard force-recreates the file (recreate=true).
	m = pressGuardKey(t, m, 's')
	got, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("[S]ave did not recreate the file: %v", err)
	}
	if string(got) != wantContent {
		t.Fatalf("recreated content = %q, want %q", got, wantContent)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// m.err de-stick (the focus-trap root cause)
// ─────────────────────────────────────────────────────────────────────────────

func TestErrDeStick_OnFileLoadedMsg(t *testing.T) {
	m := newTestWorkspace(t)
	m, _ = m.Update(ErrMsg{Err: errTest})
	if m.err == nil {
		t.Fatal("prerequisite: m.err not set")
	}

	m, _ = m.Update(FileLoadedMsg{Path: "a.md", Result: docstate.LoadResult{DiskContent: "hi", Recovered: "hi"}, Gen: m.loadGen})

	if m.err != nil {
		t.Fatalf("m.err not cleared after successful FileLoadedMsg: %v", m.err)
	}
}

func TestErrDeStick_OnFileSavedMsg(t *testing.T) {
	m := dirtyWorkspace(t)
	m = typeSeq(m, 'a')

	m, _ = m.Update(ErrMsg{Err: errTest})
	if m.err == nil {
		t.Fatal("prerequisite: m.err not set")
	}

	m = save(m)

	if m.err != nil {
		t.Fatalf("m.err not cleared after successful FileSavedMsg: %v", m.err)
	}
}

func TestErrDeStick_OnDirLoadedMsg(t *testing.T) {
	m := newTestWorkspace(t)
	m, _ = m.Update(ErrMsg{Err: errTest})
	if m.err == nil {
		t.Fatal("prerequisite: m.err not set")
	}

	m, _ = m.Update(filetree.DirLoadedMsg{Root: "/test", Entries: nil})

	if m.err != nil {
		t.Fatalf("m.err not cleared after successful DirLoadedMsg: %v", m.err)
	}
}

func TestErrDeStick_OnDirReloadedMsg(t *testing.T) {
	m := newTestWorkspace(t)
	m, _ = m.Update(ErrMsg{Err: errTest})
	if m.err == nil {
		t.Fatal("prerequisite: m.err not set")
	}

	m, _ = m.Update(filetree.DirReloadedMsg{Root: "/test", Entries: nil})

	if m.err != nil {
		t.Fatalf("m.err not cleared after successful DirReloadedMsg: %v", m.err)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Passive ticks never raise the guard — the deletion guard is event-driven
// (dirChangedMsg) ONLY. Tab focus is a pure rendering transition; the flush
// tick is the SQLite recovery-snapshot cadence. Neither may raise a guard
// (regression guards for the Esc "decide later" contract — a passive re-raise
// would re-nag the user after they dismissed the guard).
// ─────────────────────────────────────────────────────────────────────────────

func TestDeletedGuard_TabFocusDoesNotRaiseGuard(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	if err := os.WriteFile(path, []byte("hello"), 0o644); err != nil {
		t.Fatal(err)
	}

	m := deletedGuardFixture(t, path)
	if err := os.Remove(path); err != nil {
		t.Fatal(err)
	}

	// A tab-focus event for the current doc (same path → requestOpenPath is a
	// no-op) must NOT raise the deletion guard: focus is pure rendering.
	m, _ = m.Update(opentabs.TabSelectedMsg{Path: m.view.Path(), DocID: m.view.DocID()})

	if m.footer.InGuard() {
		t.Fatalf("tab focus raised a guard (kind=%v) — focus must never raise a guard", m.footer.GuardKind())
	}
	if m.pendingDeleted.active {
		t.Fatal("tab focus set pendingDeleted — focus must never trigger deletion detection")
	}
}

func TestDeletedGuard_FlushTickDoesNotRaiseGuard(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	if err := os.WriteFile(path, []byte("hello"), 0o644); err != nil {
		t.Fatal(err)
	}

	m := deletedGuardFixture(t, path)
	if err := os.Remove(path); err != nil {
		t.Fatal(err)
	}

	// The debounced flush tick (SQLite recovery-snapshot cadence, not a disk
	// autosave) must NOT raise the deletion guard — otherwise it would re-nag
	// every tick after the user pressed Esc to keep editing a deleted file.
	m, _ = m.Update(pendingFlushMsg{gen: m.flushGen})

	if m.footer.InGuard() {
		t.Fatalf("flush tick raised a guard (kind=%v) — the snapshot tick must never raise a guard", m.footer.GuardKind())
	}
	if m.pendingDeleted.active {
		t.Fatal("flush tick set pendingDeleted — persistence ticks must never trigger deletion detection")
	}
}
