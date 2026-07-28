package markdownedit

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/terminal"
	"rune/pkg/ui/components/image"
)

// runImageCmd executes cmd (and any nested tea.BatchMsg) and returns every
// leaf tea.Msg it produced — enough to pull the image.ErrorMsg out of a real
// DecodeCmd failure without reaching into the image package's internals.
func runImageCmd(cmd tea.Cmd) []tea.Msg {
	if cmd == nil {
		return nil
	}
	msg := cmd()
	if batch, ok := msg.(tea.BatchMsg); ok {
		var out []tea.Msg
		for _, c := range batch {
			out = append(out, runImageCmd(c)...)
		}
		return out
	}
	if msg == nil {
		return nil
	}
	return []tea.Msg{msg}
}

// TestFailedImageRetriesOnlyOnMtimeChange locks in M3's retry rule: a Failed
// instance is sticky while the file's mtime is unchanged (no retry storm on
// a permanently-broken file), but a genuine mtime change (e.g. `touch`, or a
// rewrite) always respawns and retries — even though the prior state was
// Failed, not Live.
func TestFailedImageRetriesOnlyOnMtimeChange(t *testing.T) {
	tmpDir := t.TempDir()
	imgFile := filepath.Join(tmpDir, "photo.webp")
	if err := os.WriteFile(imgFile, []byte("RIFF"), 0o644); err != nil {
		t.Fatalf("create stub image: %v", err)
	}

	m := newImagePipelineModel(t, terminal.TermCaps{GraphicsProtocol: terminal.GraphicsKitty, TrueColor: true})
	// Unfocused: the cursor-on-embed-line reveal rule (§2.3) would otherwise
	// render the embed as source (Revealed), and StandaloneImagePath skips
	// Revealed spans — mirrors TestDiscoverImagesOnLoad's setup.
	m = m.SetFocused(false)
	m, _ = m.SetContent("![[photo.webp]]") // no docPath yet, so its own discovery no-ops
	m = m.SetDocPath(filepath.Join(tmpDir, "note.md"))

	m, cmd := m.syncImageSet()
	if cmd == nil {
		t.Fatal("setup: expected a decode Cmd")
	}
	failedMtime := m.images["photo.webp"].Mtime()

	// Drive the tracked instance to Failed via a REAL decode failure (the
	// stub "RIFF" bytes are not a decodable image) — Gen must match the
	// live instance's own spawn gen, which only a real Init()/decode round
	// trip stamps correctly.
	msgs := runImageCmd(m.images["photo.webp"].Init())
	var errMsg image.ErrorMsg
	found := false
	for _, msg := range msgs {
		if e, ok := msg.(image.ErrorMsg); ok {
			errMsg = e
			found = true
		}
	}
	if !found {
		t.Fatalf("expected the stub (invalid) image file to fail decode; got %#v", msgs)
	}
	m, _ = m.updateImages(errMsg)
	if m.images["photo.webp"].State() != image.Failed {
		t.Fatalf("setup: expected Failed after ErrorMsg, got %v", m.images["photo.webp"].State())
	}

	// Same mtime: rediscovery must NOT respawn (no Cmd, same instance).
	before := m.images["photo.webp"]
	m, cmd = m.syncImageSet()
	if cmd != nil {
		t.Fatalf("same-mtime Failed instance must not respawn, got a Cmd")
	}
	if m.images["photo.webp"].State() != before.State() {
		t.Fatalf("same-mtime Failed instance must be left untouched")
	}

	// Change the mtime (simulates `touch`/rewrite) — rediscovery must respawn
	// and retry: a fresh PendingDecode instance with a decode Cmd.
	newMtime := time.Unix(0, failedMtime+int64(time.Second))
	if err := os.Chtimes(imgFile, newMtime, newMtime); err != nil {
		t.Fatalf("chtimes: %v", err)
	}
	m, cmd = m.syncImageSet()
	if cmd == nil {
		t.Fatal("mtime-changed Failed instance must respawn with a decode Cmd")
	}
	if m.images["photo.webp"].State() != image.PendingDecode {
		t.Fatalf("respawned instance must start PendingDecode, got %v", m.images["photo.webp"].State())
	}
}

// TestImageErrorSurfacesOnFailedTransition locks in M3's error-surfacing
// rule: the ¬Failed->Failed edge in updateImages must emit an ImageErrorMsg
// (E5) so the workspace's existing errorCmd chokepoint can show it — a real
// decode failure (unreadable file) must not vanish silently as it did before
// M2/M3 (pressure point #1).
func TestImageErrorSurfacesOnFailedTransition(t *testing.T) {
	tmpDir := t.TempDir()
	// An existing file that resolves but fails to decode (not a real image) —
	// resolveEmbed needs the path to exist; the decode failure is what drives
	// the ¬Failed->Failed edge.
	imgFile := filepath.Join(tmpDir, "corrupt.webp")
	if err := os.WriteFile(imgFile, []byte("not an image"), 0o644); err != nil {
		t.Fatalf("create stub image: %v", err)
	}

	m := newImagePipelineModel(t, terminal.TermCaps{GraphicsProtocol: terminal.GraphicsKitty, TrueColor: true})
	m = m.SetFocused(false)
	m, _ = m.SetContent("![[corrupt.webp]]") // no docPath yet, so its own discovery no-ops
	m = m.SetDocPath(filepath.Join(tmpDir, "note.md"))

	m, cmd := m.syncImageSet()
	if cmd == nil {
		t.Fatal("setup: expected a decode Cmd for the embed")
	}
	msgs := runImageCmd(cmd)
	var errMsg image.ErrorMsg
	found := false
	for _, msg := range msgs {
		if e, ok := msg.(image.ErrorMsg); ok {
			errMsg = e
			found = true
		}
	}
	if !found {
		t.Fatalf("expected a decode failure for a corrupt file; got %#v", msgs)
	}

	m, updCmd := m.updateImages(errMsg)
	if m.images["corrupt.webp"].State() != image.Failed {
		t.Fatalf("expected Failed, got %v", m.images["corrupt.webp"].State())
	}
	if updCmd == nil {
		t.Fatal("expected updateImages to return a Cmd surfacing the error")
	}
	surfaced := runImageCmd(updCmd)
	gotErrMsg := false
	for _, msg := range surfaced {
		if _, ok := msg.(ImageErrorMsg); ok {
			gotErrMsg = true
		}
	}
	if !gotErrMsg {
		t.Fatalf("expected an ImageErrorMsg among %#v", surfaced)
	}
}
