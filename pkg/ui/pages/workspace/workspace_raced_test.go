package workspace

import (
	"os"
	"path/filepath"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/editor/buffer"
	"rune/pkg/ui/components/footer"
	"rune/pkg/vfs"
)

// racedHookFS wraps a vfs.FS and injects an external write immediately
// before Exchange, simulating a writer racing Materialize's atomic swap
// window — the workspace-level counterpart to docstate's internal hookFS.
// This is the plan's "Hook-FS test" for F5, exercised through the REAL
// interactive-save input path (startSave -> materializeStoreCmd ->
// FileSavedMsg -> handleFileSavedMsg), not an internal setter.
type racedHookFS struct {
	vfs.FS
	path       string
	racedBytes []byte
	fired      bool
}

func (h *racedHookFS) Exchange(a, b string) error {
	if !h.fired {
		h.fired = true
		if err := h.FS.WriteFile(h.path, h.racedBytes, 0o644); err != nil {
			return err
		}
	}
	return h.FS.Exchange(a, b)
}

// TestSaveRace_RaisesRacedGuardAndRestoreTheirsRoundTrips is the workspace
// half of the F5 regression (WP-R3's "Hook-FS test"): a save races a
// concurrent external writer inside Materialize's atomic-swap window.
// Asserts (d) the workspace guard raises as GuardRaced with theirs ==
// the displaced blob content, and (e) [R]estore-theirs round-trips the
// external bytes back to disk. (a)/(b)/(c) — the displaced blob exists, the
// temp is removed, and saved_obs matches our just-written bytes — are
// covered at the store level by
// docstate.TestMaterialize_InWindowRace_CommitsRacedWithCapture.
func TestSaveRace_RaisesRacedGuardAndRestoreTheirsRoundTrips(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	const original = "original"
	const ours = "our new content"
	const raced = "raced writer content"
	if err := os.WriteFile(path, []byte(original), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	hook := &racedHookFS{FS: vfs.Disk{}, path: path, racedBytes: []byte(raced)}
	m = m.WithFS(hook)
	m = loadFile(m, path, original)
	docID := m.view.DocID()
	if docID == 0 {
		t.Fatal("store not available")
	}

	// A real journaled edit, mirroring diverge() elsewhere in this package.
	if _, err := m.store.AppendEdit(docID,
		[]buffer.AppliedEdit{{Start: 0, End: len(original), Deleted: original, Insert: ours}}, nil, nil); err != nil {
		t.Fatalf("AppendEdit: %v", err)
	}
	m.editor = m.editor.SetContent(ours)

	m, cmd := m.startSave()
	m = settle(t, m, cmd)

	if !hook.fired {
		t.Fatal("setup: race hook never fired")
	}

	// (d) workspace guard raised with theirs == displaced blob content.
	if !m.footer.InGuard() || m.footer.GuardKind() != footer.GuardRaced {
		t.Fatalf("expected GuardRaced raised; inGuard=%v kind=%v", m.footer.InGuard(), m.footer.GuardKind())
	}
	if !m.pendingRaced.active {
		t.Fatal("expected pendingRaced active")
	}
	theirsContent, err := m.store.GetBlob(m.pendingRaced.fresh.BlobHash)
	if err != nil {
		t.Fatalf("GetBlob(displaced): %v", err)
	}
	if theirsContent != raced {
		t.Fatalf("pendingRaced.fresh content = %q, want the displaced %q", theirsContent, raced)
	}

	// Disk currently holds OUR bytes — the swap physically already happened.
	diskNow, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if string(diskNow) != ours {
		t.Fatalf("disk after raced commit = %q, want OUR content %q", diskNow, ours)
	}

	// (e) [R]estore-theirs round-trips the external bytes back to disk — a
	// REAL keypress ('r'), routed through the guard-priority key handling
	// exactly as a user would trigger it.
	m, cmd = m.Update(tea.KeyPressMsg{Code: 'r', Text: "r"})
	m = settle(t, m, cmd)

	diskAfter, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if string(diskAfter) != raced {
		t.Fatalf("disk after restore-theirs = %q, want the restored displaced content %q", diskAfter, raced)
	}
	if m.pendingRaced.active {
		t.Fatal("expected pendingRaced cleared after restore-theirs")
	}
	if m.footer.InGuard() {
		t.Fatal("expected the Raced guard cleared after restore-theirs")
	}
}

// TestSaveRace_KeepMineClearsGuardWithoutFurtherWrite: [K]eep-mine dismisses
// the guard without any further disk write — our already-committed bytes
// stand.
func TestSaveRace_KeepMineClearsGuardWithoutFurtherWrite(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	const original = "original"
	const ours = "our new content"
	const raced = "raced writer content"
	if err := os.WriteFile(path, []byte(original), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	hook := &racedHookFS{FS: vfs.Disk{}, path: path, racedBytes: []byte(raced)}
	m = m.WithFS(hook)
	m = loadFile(m, path, original)
	docID := m.view.DocID()
	if docID == 0 {
		t.Fatal("store not available")
	}
	if _, err := m.store.AppendEdit(docID,
		[]buffer.AppliedEdit{{Start: 0, End: len(original), Deleted: original, Insert: ours}}, nil, nil); err != nil {
		t.Fatalf("AppendEdit: %v", err)
	}
	m.editor = m.editor.SetContent(ours)

	m, cmd := m.startSave()
	m = settle(t, m, cmd)
	if !m.pendingRaced.active {
		t.Fatal("setup: expected pendingRaced active")
	}

	m, cmd = m.Update(tea.KeyPressMsg{Code: 'k', Text: "k"})
	m = settle(t, m, cmd)

	if m.pendingRaced.active || m.footer.InGuard() {
		t.Fatal("expected the Raced guard cleared after keep-mine")
	}
	diskAfter, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if string(diskAfter) != ours {
		t.Fatalf("keep-mine must not touch disk further: got %q, want unchanged OUR content %q", diskAfter, ours)
	}
}
