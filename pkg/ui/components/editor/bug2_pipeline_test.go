package editor

import (
	"path/filepath"
	"testing"

	"rune/pkg/editor/display"
	"rune/pkg/terminal"
)

// inlineEditor builds a WezTerm-style (inline-image-capable) editor whose open
// file lives in dir, with the given content and cursor on line 0.
func inlineEditor(t *testing.T, dir, content string) Model {
	t.Helper()
	m := newTestEditor("")
	m.termCaps = terminal.TermCaps{GraphicsProtocol: terminal.GraphicsWezTerm, TrueColor: true}
	m = m.SetContent(filepath.Join(dir, "note.md"), []byte(content))
	m = m.SetSize(80, 24)
	m = m.SetFocused(true)
	return m
}

// TestBug2_MarkdownImageFoldsAndDiscovers is the WP0 diagnostic gate. It drives
// the REAL parse→fold→discover pipeline for a standalone markdown image that
// sits on a line AFTER line 0, with the cursor on line 0. If this passes, BUG2
// is confirmed identical to BUG1: the image folds to a Rendered span with an
// ImagePath and is discovered/decoded — only the final paint (delivery) fails.
func TestBug2_MarkdownImageFoldsAndDiscovers(t *testing.T) {
	dir := t.TempDir()
	// A real file at the resolved location (basename → note's directory).
	writePNG(t, filepath.Join(dir, "image.webp"), 80, 80)

	m := inlineEditor(t, dir, "text\n![alt](image.webp)\nmore")

	// The image line (line 1) must carry exactly one image span, folded
	// (Rendered) with a non-empty ImagePath. Cursor is on line 0, so the image
	// span must NOT be revealed.
	if len(m.snapshot.Lines) < 2 {
		t.Fatalf("expected at least 2 display lines, got %d", len(m.snapshot.Lines))
	}
	imgLine := m.snapshot.Lines[1]

	imageSpans := 0
	for _, sp := range imgLine.Spans {
		if sp.Kind == display.TokenImage {
			imageSpans++
			if sp.State != display.Rendered {
				t.Errorf("image span state = %v, want Rendered (cursor is on line 0)", sp.State)
			}
			if sp.ImagePath == "" {
				t.Error("image span ImagePath is empty, want non-empty")
			}
		}
	}
	if imageSpans != 1 {
		t.Fatalf("expected exactly 1 TokenImage span on the image line, got %d (spans=%+v)", imageSpans, imgLine.Spans)
	}

	// The line must qualify as a standalone image line.
	path, ok := display.StandaloneImagePath(imgLine)
	if !ok {
		t.Fatal("StandaloneImagePath reported not-standalone for a lone markdown image")
	}
	if path == "" {
		t.Fatal("StandaloneImagePath returned an empty path")
	}

	// Discovery must register a pendingDecode entry and return a decode Cmd.
	m, cmd := m.discoverNewImages()
	if cmd == nil {
		t.Fatal("discoverNewImages returned nil Cmd; expected a decode for the markdown image")
	}
	if e, ok := m.images.get(path); !ok || e.state != pendingDecode {
		t.Fatalf("expected pendingDecode registry entry for %q, got %+v ok=%v", path, e, ok)
	}
}
