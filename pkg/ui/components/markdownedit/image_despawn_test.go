package markdownedit

import (
	"os"
	"path/filepath"
	"testing"

	"rune/pkg/editor/buffer"
	"rune/pkg/terminal"
	"rune/pkg/ui/components/image"
)

// TestDeleteEmbedLineDespawnsImage locks in M6's despawn rule: deleting the
// only line referencing an image removes it from the tracked instance set
// entirely (present, computed fresh from the syntax spans every syncImageSet
// pass, no longer contains the path) — the images map no longer only grows
// (pressure point #5).
func TestDeleteEmbedLineDespawnsImage(t *testing.T) {
	tmpDir := t.TempDir()
	imgFile := filepath.Join(tmpDir, "photo.webp")
	if err := os.WriteFile(imgFile, []byte("RIFF"), 0o644); err != nil {
		t.Fatalf("create stub image: %v", err)
	}

	m := newImagePipelineModel(t, terminal.TermCaps{GraphicsProtocol: terminal.GraphicsKitty, TrueColor: true})
	m = m.SetFocused(false)
	m = m.SetDocPath(filepath.Join(tmpDir, "note.md"))

	const embedLine = "![[photo.webp]]\n"
	m, cmd := m.SetContent(embedLine + "B\nC")
	if cmd == nil {
		t.Fatal("setup: expected a decode Cmd")
	}
	if _, tracked := m.images["photo.webp"]; !tracked {
		t.Fatal("setup: image not tracked after SetContent")
	}

	// Delete the embed line (including its trailing newline) — the only
	// reference to "photo.webp" anywhere in the document.
	m, _, err := m.ReplaceRange(0, len(embedLine), "")
	if err != nil {
		t.Fatalf("delete embed line: %v", err)
	}
	if _, tracked := m.images["photo.webp"]; tracked {
		t.Fatal("expected image to be despawned after deleting its only embed line")
	}
}

// TestUndoRespawnsDespawnedImage locks in the other half of M6's despawn
// rule: undoing the delete restores the embed line, and the next reconcile
// pass respawns the image as a fresh (PendingDecode) instance — despawn is
// not a one-way ratchet.
func TestUndoRespawnsDespawnedImage(t *testing.T) {
	tmpDir := t.TempDir()
	imgFile := filepath.Join(tmpDir, "photo.webp")
	if err := os.WriteFile(imgFile, []byte("RIFF"), 0o644); err != nil {
		t.Fatalf("create stub image: %v", err)
	}

	m := newImagePipelineModel(t, terminal.TermCaps{GraphicsProtocol: terminal.GraphicsKitty, TrueColor: true})
	m = m.SetFocused(false)
	m = m.SetDocPath(filepath.Join(tmpDir, "note.md"))

	const embedLine = "![[photo.webp]]\n"
	m, cmd := m.SetContent(embedLine + "B\nC")
	if cmd == nil {
		t.Fatal("setup: expected a decode Cmd")
	}

	m, _, err := m.ReplaceRange(0, len(embedLine), "")
	if err != nil {
		t.Fatalf("delete embed line: %v", err)
	}
	if _, tracked := m.images["photo.webp"]; tracked {
		t.Fatal("setup: expected image despawned after delete")
	}

	var edits []buffer.AppliedEdit
	m, edits = m.DrainEdits()
	if len(edits) == 0 {
		t.Fatal("expected the delete to have produced a journaled edit")
	}
	m, _, err = m.ApplyInverse(edits)
	if err != nil {
		t.Fatalf("undo: %v", err)
	}
	if _, tracked := m.images["photo.webp"]; !tracked {
		t.Fatal("expected image to respawn after undo restored its embed line")
	}
	if got := m.images["photo.webp"].State(); got != image.PendingDecode {
		t.Fatalf("respawned instance must start PendingDecode, got %v", got)
	}
}
