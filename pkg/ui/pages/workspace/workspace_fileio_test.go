package workspace

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/docstate"
	"rune/pkg/vfs"
)

// tabHasDocID reports whether any open tab carries the given VFS doc id.
func tabHasDocID(m Model, id int64) bool {
	for i := 0; i < m.opentabs.Len(); i++ {
		if m.opentabs.DocIDAt(i) == id {
			return true
		}
	}
	return false
}

// newMaterializeTestStore returns a real, disk-backed Store for a single
// materializeStoreCmd unit test, plus the DocRef bound to path.
func newMaterializeTestStore(t *testing.T, path, seedContent string) (*docstate.Store, docstate.DocRef) {
	t.Helper()
	store := docstate.NewTestStore(t)
	store.UseFS(vfs.Disk{})
	if seedContent != "" {
		if err := os.WriteFile(path, []byte(seedContent), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	ref, err := store.CreateScratch("Untitled")
	if err != nil {
		t.Fatalf("CreateScratch: %v", err)
	}
	if err := store.Bind(ref.ID, path); err != nil {
		t.Fatalf("Bind: %v", err)
	}
	return store, ref
}

// TestMaterialize_BindNewRefusesClobber: naming an untitled over an existing
// file must NOT overwrite it (Catastrophic, rung 1 — CLAUDE.md §1.4.1). Uses
// a genuinely never-bound scratch doc (`documents.path == ""` going in) — the
// real production shape (workspace_update.go's RenameRequestMsg handler never
// binds before the first materializeStoreCmd) — rather than pre-binding via
// store.Bind, which would paper over a bind-new path-resolution bug.
func TestMaterialize_BindNewRefusesClobber(t *testing.T) {
	path := filepath.Join(t.TempDir(), "exists.md")
	if err := os.WriteFile(path, []byte("original"), 0o644); err != nil {
		t.Fatal(err)
	}
	store := docstate.NewTestStore(t)
	store.UseFS(vfs.Disk{})
	defer store.Close()
	ref, err := store.CreateScratch("Untitled")
	if err != nil {
		t.Fatalf("CreateScratch: %v", err)
	}

	// bindNew=true — RenameExcl's own no-clobber check refuses.
	msg := materializeStoreCmd(store, ref.ID, path, "new content", 0, 0, "r1", true)()
	e, ok := msg.(FileSaveErrorMsg)
	if !ok || !e.Conflict {
		t.Fatalf("expected conflict FileSaveErrorMsg, got %#v", msg)
	}
	if b, _ := os.ReadFile(path); string(b) != "original" {
		t.Fatalf("bind-new clobbered an existing file: %q", b)
	}
}

// TestMaterialize_OverwriteRefusesExternalChange: ⌘S must refuse to clobber a
// file that changed on disk since it was loaded (§1.4.7 — content-hash CAS).
func TestMaterialize_OverwriteRefusesExternalChange(t *testing.T) {
	path := filepath.Join(t.TempDir(), "f.md")
	store, ref := newMaterializeTestStore(t, path, "v1")
	defer store.Close()
	loaded, err := store.Load(path)
	if err != nil {
		t.Fatal(err)
	}
	expect, _, err := store.SavedObs(loaded.DocID)
	if err != nil {
		t.Fatal(err)
	}

	// Simulate an external editor changing the file.
	if err := os.WriteFile(path, []byte("v2 external longer"), 0o644); err != nil {
		t.Fatal(err)
	}
	msg := materializeStoreCmd(store, ref.ID, path, "mine", expect.ID, 0, "r1", false)()
	e, ok := msg.(FileSaveErrorMsg)
	if !ok || !e.Conflict {
		t.Fatalf("expected conflict FileSaveErrorMsg, got %#v", msg)
	}
	if b, _ := os.ReadFile(path); string(b) != "v2 external longer" {
		t.Fatalf("overwrite clobbered an external change: %q", b)
	}
}

// TestMaterialize_OverwriteRefusesVanishedTarget: ⌘S must refuse to silently
// recreate a file that vanished (renamed away or deleted) since it was
// opened, when bindNew is false (an ordinary overwrite-intent save) — never
// silently (re)create it (§1.4.4). Reachable via a foreground tab whose file
// is renamed away externally, quit's "save all," and tab eviction — all three
// share this write primitive. Silently recreating it would either resurrect a
// deleted file unasked, or — if renamed — fork the document's identity
// (§1.4.6): Store.Bind would claim a fresh inode at this now-stale path while
// the file's true continuation (new path, original inode) becomes an orphan
// next time anything opens it.
func TestMaterialize_OverwriteRefusesVanishedTarget(t *testing.T) {
	path := filepath.Join(t.TempDir(), "f.md")
	store, ref := newMaterializeTestStore(t, path, "v1")
	defer store.Close()
	loaded, err := store.Load(path)
	if err != nil {
		t.Fatal(err)
	}
	expect, _, err := store.SavedObs(loaded.DocID)
	if err != nil {
		t.Fatal(err)
	}

	// The file vanishes (renamed away or deleted) after load.
	if err := os.Remove(path); err != nil {
		t.Fatal(err)
	}
	msg := materializeStoreCmd(store, ref.ID, path, "mine", expect.ID, 0, "r1", false)()
	e, ok := msg.(FileSaveErrorMsg)
	if !ok || !e.Missing {
		t.Fatalf("expected Missing FileSaveErrorMsg, got %#v", msg)
	}
	if _, err := os.Stat(path); err == nil {
		t.Fatal("materializeStoreCmd must not have recreated the vanished file")
	}
}

// TestMaterialize_OverwriteWritesVerbatim: writes the bytes verbatim — no
// line-ending/trailing-newline normalization (§1.4.5).
func TestMaterialize_OverwriteWritesVerbatim(t *testing.T) {
	path := filepath.Join(t.TempDir(), "f.md")
	store, ref := newMaterializeTestStore(t, path, "v1")
	defer store.Close()
	loaded, err := store.Load(path)
	if err != nil {
		t.Fatal(err)
	}
	expect, _, err := store.SavedObs(loaded.DocID)
	if err != nil {
		t.Fatal(err)
	}

	const want = "line1\r\nline2 no trailing nl"
	msg := materializeStoreCmd(store, ref.ID, path, want, expect.ID, 0, "r1", false)()
	if _, ok := msg.(FileSavedMsg); !ok {
		t.Fatalf("expected FileSavedMsg, got %#v", msg)
	}
	if b, _ := os.ReadFile(path); string(b) != want {
		t.Fatalf("bytes not written verbatim: %q", b)
	}
}

// TestRename_RefusesClobber: renaming onto an existing file must NOT destroy it.
func TestRename_RefusesClobber(t *testing.T) {
	dir := t.TempDir()
	a, b := filepath.Join(dir, "a.md"), filepath.Join(dir, "b.md")
	if err := os.WriteFile(a, []byte("A"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(b, []byte("B"), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, ok := fileRenameCmd(vfs.Disk{}, a, b)().(FileRenameErrorMsg); !ok {
		t.Fatalf("expected FileRenameErrorMsg when target exists")
	}
	if bb, _ := os.ReadFile(b); string(bb) != "B" {
		t.Fatalf("rename clobbered target: %q", bb)
	}
}

// raceyRenameFS wraps a vfs.FS and creates newPath (simulating a concurrent
// creator) immediately before delegating to the real RenameExcl — the F9
// regression: a Stat-then-Rename implementation checked newPath BEFORE this
// race could land, so it would have clobbered the racer's bytes; RenameExcl
// is atomic, so the SAME race lands INSIDE the primitive itself and is still
// refused.
type raceyRenameFS struct {
	vfs.FS
	newPath string
	racer   []byte
	fired   bool
}

func (h *raceyRenameFS) RenameExcl(oldPath, newPath string) error {
	if !h.fired && newPath == h.newPath {
		h.fired = true
		if err := h.FS.WriteFile(newPath, h.racer, 0o644); err != nil {
			return err
		}
	}
	return h.FS.RenameExcl(oldPath, newPath)
}

// TestFileRenameCmd_RenameExclClosesTOCTOU is the F9 regression: a
// concurrent creator lands INSIDE the rename primitive's own no-clobber
// window (not merely before an upfront Stat) — RenameExcl still refuses,
// closing the window a Stat-then-Rename implementation left open (G1).
func TestFileRenameCmd_RenameExclClosesTOCTOU(t *testing.T) {
	dir := t.TempDir()
	oldPath := filepath.Join(dir, "old.md")
	newPath := filepath.Join(dir, "new.md")
	if err := os.WriteFile(oldPath, []byte("original"), 0o644); err != nil {
		t.Fatal(err)
	}

	hook := &raceyRenameFS{FS: vfs.Disk{}, newPath: newPath, racer: []byte("racer bytes")}
	msg := fileRenameCmd(hook, oldPath, newPath)()

	if !hook.fired {
		t.Fatal("setup: race hook never fired")
	}
	if _, ok := msg.(FileRenameErrorMsg); !ok {
		t.Fatalf("expected FileRenameErrorMsg (no-clobber refusal), got %#v", msg)
	}
	got, err := os.ReadFile(newPath)
	if err != nil {
		t.Fatalf("ReadFile(newPath): %v", err)
	}
	if string(got) != "racer bytes" {
		t.Fatalf("racer's bytes clobbered: got %q, want unchanged %q", got, "racer bytes")
	}
	// oldPath must survive a refused rename.
	if _, err := os.ReadFile(oldPath); err != nil {
		t.Fatalf("oldPath should survive a refused rename: %v", err)
	}
}

// TestSwitchAwayAndBack_PreservesUnsavedEdit pins a severe bug found by
// FuzzHumanSession: stampLoadBaseline used to blindly re-stamp a 'disk'
// snapshot on EVERY re-open, even when nothing on disk had changed. That
// redundant snapshot shares its seq with the 'switch' snapshot forceSnapshot
// takes when navigating away (both stamped at CurrentSeq, since no new edits
// land in between) but holds STALE, un-journaled disk bytes — and being
// inserted later, it wins RecoverDocument's id-DESC same-seq tie-break,
// silently regressing the reconstructed buffer to drop the user's unsaved
// edit. Reachable via completely ordinary tab-switching — no external change
// or save involved at all.
func TestSwitchAwayAndBack_PreservesUnsavedEdit(t *testing.T) {
	dir := t.TempDir()
	pathA := filepath.Join(dir, "a.md")
	pathB := filepath.Join(dir, "b.md")
	if err := os.WriteFile(pathA, []byte("original"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(pathB, []byte("b-content"), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, pathA, "original")
	docIDA := m.view.DocID()
	if docIDA == 0 {
		t.Fatal("expected a real docID for A")
	}

	m = focusEditor(m)
	m, _ = m.Update(tea.KeyPressMsg{Code: 'x', Text: "x"})
	if got := m.editor.Content(); got != "xoriginal" {
		t.Fatalf("setup: editor content = %q, want %q", got, "xoriginal")
	}

	// Switch to B (unsaved edit on A) and back to A — real navigation, so
	// forceSnapshot fires for A on the way out. No external change, no save.
	m, cmd := m.requestOpenPath(0, pathB)
	m = drainCmd(m, cmd)
	if m.view.Path() != pathB {
		t.Fatalf("setup: expected view=B, got %q", m.view.Path())
	}
	m, cmd = m.requestOpenPath(docIDA, pathA)
	m = drainCmd(m, cmd)

	if got := m.editor.Content(); got != "xoriginal" {
		t.Fatalf("switching away and back lost the unsaved edit: editor content = %q, want %q", got, "xoriginal")
	}
	got, err := m.store.RecoverDocument(docIDA)
	if err != nil {
		t.Fatalf("RecoverDocument: %v", err)
	}
	if got != "xoriginal" {
		t.Fatalf("RecoverDocument = %q, want %q (the journaled edit)", got, "xoriginal")
	}
}

// TestSave_RebindsAcrossChurnedInode is a deterministic sibling of docstate's
// TestBind_StableAcrossInodeChange (which must t.Skip if the OS happens to
// reuse an inode on real disk). Using a vfs.Mem configured to churn inode on
// every write — matching a real save's publish step (Exchange/RenameExcl),
// unlike Mem's stable-by-default behavior (§1.4.1/§1.4.6) — this drives two
// real save cycles through Update() and asserts the docID and history survive both, the
// exact regression class ("a stray identity lookup sneaks in before Bind on
// some save path") the fuzzer's default vfs.Mem can never reach, since it
// never churns inode after a path's first write.
func TestSave_RebindsAcrossChurnedInode(t *testing.T) {
	mem := vfs.NewMem(vfs.WithChurnInodeOnWrite())
	const path = "/test/note.md"
	if err := mem.WriteFile(path, []byte("v1"), 0o644); err != nil {
		t.Fatal(err)
	}

	store, err := docstate.OpenInMemory(time.Now)
	if err != nil {
		t.Fatal(err)
	}
	defer store.Close()
	store.UseFS(mem)

	m := newTestWorkspace(t).WithFS(mem)
	var cmd tea.Cmd
	m, cmd = m.Update(StoreReadyMsg{Store: store})
	m = drainCmd(m, cmd)

	m = loadFile(m, path, "v1")
	docID := m.view.DocID()
	if docID == 0 {
		t.Fatal("expected a real docID")
	}

	m = focusEditor(m)
	for i := 1; i <= 2; i++ {
		m, _ = m.Update(tea.KeyPressMsg{Code: 'x', Text: "x"})
		m, cmd = m.startSave()
		m = drainCmd(m, cmd)

		if got := m.view.DocID(); got != docID {
			t.Fatalf("save #%d: docID changed %d -> %d (churned inode orphaned history)", i, docID, got)
		}
		if has, err := m.store.HasHistory(docID); err != nil || !has {
			t.Fatalf("save #%d: HasHistory(%d) = %v, %v", i, docID, has, err)
		}
		ref, err := m.store.OpenPath(path)
		if err != nil || ref.ID != docID {
			t.Fatalf("save #%d: OpenPath(%q) = %+v, %v; want docID %d", i, path, ref, err, docID)
		}
	}
}

// TestRestore_OnlyNonEmptyGenuineScratch pins the launch-recovery scoping: a
// prior-session untitled with real content reopens, but a blank/whitespace one
// does NOT (it would otherwise clutter every launch). Combined with the docstate
// inode filter, this prevents the "stale/foreign Untitled tabs" regression.
func TestRestore_OnlyNonEmptyGenuineScratch(t *testing.T) {
	m := newTestWorkspace(t)
	store := docstate.NewTestStore(t)

	// Prior-session genuine non-empty untitled work — must be recovered.
	genuine, err := store.CreateScratch("note")
	if err != nil {
		t.Fatalf("CreateScratch genuine: %v", err)
	}
	if _, err := store.CreateSnapshot(genuine.ID, "recovered note text", 0); err != nil {
		t.Fatalf("CreateSnapshot genuine: %v", err)
	}
	// Prior-session blank untitled (whitespace only) — must NOT resurface.
	blank, err := store.CreateScratch("blank")
	if err != nil {
		t.Fatalf("CreateScratch blank: %v", err)
	}
	if _, err := store.CreateSnapshot(blank.ID, "   \n\t", 0); err != nil {
		t.Fatalf("CreateSnapshot blank: %v", err)
	}

	m, cmd := m.Update(StoreReadyMsg{Store: store})
	for _, msg := range execCmds(cmd) {
		m, _ = m.Update(msg)
	}

	if !tabHasDocID(m, genuine.ID) {
		t.Error("non-empty prior-session scratch was not recovered as a tab")
	}
	if tabHasDocID(m, blank.ID) {
		t.Error("blank prior-session scratch was wrongly recovered as a tab")
	}
}

// TestStartupUntitled_DurableAfterStoreReady pins Fix #4: the startup untitled
// (created before the store opened) gets a durable VFS doc once StoreReadyMsg
// arrives, so typed content is journaled and reconstructable — a crash no
// longer loses the session.
func TestStartupUntitled_DurableAfterStoreReady(t *testing.T) {
	m := newTestWorkspace(t)
	m = withStore(t, m)
	if m.view.DocID() == 0 {
		t.Fatal("startup untitled was not upgraded to a durable VFS doc")
	}
	m = focusEditor(m)
	m, _ = m.Update(tea.KeyPressMsg{Code: 'x', Text: "x"})
	m, _ = m.Update(tea.KeyPressMsg{Code: 'y', Text: "y"})

	got, err := m.store.RecoverDocument(m.view.DocID())
	if err != nil {
		t.Fatalf("RecoverDocument: %v", err)
	}
	if got == "" {
		t.Fatal("typed content was not journaled to the VFS")
	}
	if got != m.editor.Content() {
		t.Fatalf("VFS content %q != buffer %q", got, m.editor.Content())
	}
}
