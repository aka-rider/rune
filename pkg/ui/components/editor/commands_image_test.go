package editor

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"rune/pkg/terminal"
)

func newImageTestEditor(caps terminal.TermCaps) Model {
	m := newTestEditor("")
	m.termCaps = caps
	return m
}

func TestImagePaste_SavesFileAndProducesMsg(t *testing.T) {
	tmp := t.TempDir()
	filePath := filepath.Join(tmp, "note.md")
	os.WriteFile(filePath, []byte("# Hello"), 0o644)

	m := newImageTestEditor(terminal.TermCaps{})
	m = m.SetContent(filePath, []byte("# Hello"))
	m = m.SetSize(80, 24)

	imgData := []byte("fake-png-data-for-test")
	now := time.Date(2025, 3, 15, 10, 30, 0, 0, time.UTC)

	m, cmd := m.handleImagePaste(imgData, "image/png", now)
	if cmd == nil {
		t.Fatal("expected non-nil cmd from handleImagePaste")
	}

	// Execute the command synchronously
	msg := cmd()
	savedMsg, ok := msg.(ImageSavedMsg)
	if !ok {
		t.Fatalf("expected ImageSavedMsg, got %T: %v", msg, msg)
	}

	// Verify the file was actually written
	expectedDir := filepath.Join(tmp, "assets")
	entries, err := os.ReadDir(expectedDir)
	if err != nil {
		t.Fatalf("assets dir not created: %v", err)
	}
	if len(entries) != 1 {
		t.Fatalf("expected 1 file in assets, got %d", len(entries))
	}

	// Verify relative path format
	if !strings.HasPrefix(savedMsg.RelativePath, "assets/") {
		t.Errorf("relative path should start with assets/, got %q", savedMsg.RelativePath)
	}
	if !strings.HasSuffix(savedMsg.RelativePath, ".png") {
		t.Errorf("relative path should end with .png, got %q", savedMsg.RelativePath)
	}

	// Verify file content matches
	writtenPath := filepath.Join(tmp, savedMsg.RelativePath)
	content, err := os.ReadFile(writtenPath)
	if err != nil {
		t.Fatalf("cannot read written file: %v", err)
	}
	if string(content) != string(imgData) {
		t.Error("written file content does not match input")
	}
}

func TestImagePaste_PathTraversalRejected(t *testing.T) {
	tmp := t.TempDir()
	filePath := filepath.Join(tmp, "note.md")
	os.WriteFile(filePath, []byte("test"), 0o644)

	m := newImageTestEditor(terminal.TermCaps{})
	m = m.SetContent(filePath, []byte("test"))
	m.imageConfig.AssetsDir = "../../../etc"

	imgData := []byte("attack-data")
	now := time.Now()

	_, cmd := m.handleImagePaste(imgData, "image/png", now)
	if cmd != nil {
		t.Error("expected nil cmd for path traversal attempt")
	}
}

func TestImagePaste_AbsolutePathRejected(t *testing.T) {
	tmp := t.TempDir()
	filePath := filepath.Join(tmp, "note.md")
	os.WriteFile(filePath, []byte("test"), 0o644)

	m := newImageTestEditor(terminal.TermCaps{})
	m = m.SetContent(filePath, []byte("test"))
	m.imageConfig.AssetsDir = "/tmp/evil"

	imgData := []byte("attack-data")
	now := time.Now()

	_, cmd := m.handleImagePaste(imgData, "image/png", now)
	if cmd != nil {
		t.Error("expected nil cmd for absolute path attempt")
	}
}

func TestImagePaste_DotDotInMiddleRejected(t *testing.T) {
	tmp := t.TempDir()
	filePath := filepath.Join(tmp, "note.md")
	os.WriteFile(filePath, []byte("test"), 0o644)

	m := newImageTestEditor(terminal.TermCaps{})
	m = m.SetContent(filePath, []byte("test"))
	m.imageConfig.AssetsDir = "assets/../../../etc"

	imgData := []byte("attack-data")
	now := time.Now()

	_, cmd := m.handleImagePaste(imgData, "image/png", now)
	if cmd != nil {
		t.Error("expected nil cmd for path with traversal in middle")
	}
}

func TestImagePaste_EmptyDataIgnored(t *testing.T) {
	m := newImageTestEditor(terminal.TermCaps{})
	m = m.SetContent("/tmp/note.md", []byte("test"))

	_, cmd := m.handleImagePaste(nil, "image/png", time.Now())
	if cmd != nil {
		t.Error("expected nil cmd for empty image data")
	}

	_, cmd = m.handleImagePaste([]byte{}, "image/png", time.Now())
	if cmd != nil {
		t.Error("expected nil cmd for zero-length image data")
	}
}

func TestImagePaste_NoFilePathIgnored(t *testing.T) {
	m := newImageTestEditor(terminal.TermCaps{})
	// No SetContent — filePath is empty

	imgData := []byte("some-data")
	_, cmd := m.handleImagePaste(imgData, "image/png", time.Now())
	if cmd != nil {
		t.Error("expected nil cmd when no file is open")
	}
}

func TestImagePaste_RelativePathInMsg(t *testing.T) {
	tmp := t.TempDir()
	filePath := filepath.Join(tmp, "docs", "note.md")
	os.MkdirAll(filepath.Dir(filePath), 0o755)
	os.WriteFile(filePath, []byte("test"), 0o644)

	m := newImageTestEditor(terminal.TermCaps{})
	m = m.SetContent(filePath, []byte("test"))
	m.imageConfig.AssetsDir = "img"

	imgData := []byte("test-image-bytes")
	now := time.Date(2025, 6, 1, 12, 0, 0, 0, time.UTC)

	_, cmd := m.handleImagePaste(imgData, "image/jpeg", now)
	if cmd == nil {
		t.Fatal("expected non-nil cmd")
	}

	msg := cmd()
	savedMsg, ok := msg.(ImageSavedMsg)
	if !ok {
		t.Fatalf("expected ImageSavedMsg, got %T", msg)
	}

	// Must be relative (no leading /)
	if filepath.IsAbs(savedMsg.RelativePath) {
		t.Errorf("path should be relative, got %q", savedMsg.RelativePath)
	}
	// Must start with configured assets dir
	if !strings.HasPrefix(savedMsg.RelativePath, "img/") {
		t.Errorf("expected prefix img/, got %q", savedMsg.RelativePath)
	}
	// Must have jpg extension
	if !strings.HasSuffix(savedMsg.RelativePath, ".jpg") {
		t.Errorf("expected .jpg extension, got %q", savedMsg.RelativePath)
	}
}

func TestRenderImageFallback_NoGraphics(t *testing.T) {
	caps := terminal.TermCaps{GraphicsProtocol: terminal.GraphicsNone}

	result := renderImageFallback("my screenshot", caps)
	if result != "[image: my screenshot]" {
		t.Errorf("unexpected fallback: %q", result)
	}
}

func TestRenderImageFallback_EmptyAlt(t *testing.T) {
	caps := terminal.TermCaps{GraphicsProtocol: terminal.GraphicsNone}

	result := renderImageFallback("", caps)
	if result != "[image: image]" {
		t.Errorf("unexpected fallback for empty alt: %q", result)
	}
}

func TestRenderImageFallback_WithGraphicsSupport(t *testing.T) {
	// Even with graphics support, current implementation falls through to text
	caps := terminal.TermCaps{GraphicsProtocol: terminal.GraphicsKitty}

	result := renderImageFallback("photo", caps)
	// Current implementation falls through to text fallback
	if result != "[image: photo]" {
		t.Errorf("unexpected result with graphics: %q", result)
	}
}

func TestGenerateImageFilename_Deterministic(t *testing.T) {
	data := []byte("test-image-data")
	now := time.Date(2025, 3, 15, 10, 30, 45, 0, time.UTC)

	name1 := generateImageFilename(data, now, ".png")
	name2 := generateImageFilename(data, now, ".png")

	if name1 != name2 {
		t.Errorf("same inputs should produce same filename: %q vs %q", name1, name2)
	}

	// Verify format: timestamp-hash.ext
	if !strings.HasPrefix(name1, "20250315-103045-") {
		t.Errorf("unexpected timestamp prefix: %q", name1)
	}
	if !strings.HasSuffix(name1, ".png") {
		t.Errorf("unexpected extension: %q", name1)
	}
}

func TestGenerateImageFilename_DifferentDataDifferentName(t *testing.T) {
	now := time.Date(2025, 3, 15, 10, 30, 45, 0, time.UTC)

	name1 := generateImageFilename([]byte("data-A"), now, ".png")
	name2 := generateImageFilename([]byte("data-B"), now, ".png")

	if name1 == name2 {
		t.Error("different data should produce different filenames")
	}
}

func TestExtensionForMIME(t *testing.T) {
	tests := []struct {
		mime string
		ext  string
	}{
		{"image/png", ".png"},
		{"image/jpeg", ".jpg"},
		{"image/jpg", ".jpg"},
		{"image/gif", ".gif"},
		{"image/webp", ".webp"},
		{"image/svg+xml", ".svg"},
		{"application/octet-stream", ".png"}, // unknown defaults to png
		{"", ".png"},
	}
	for _, tt := range tests {
		t.Run(tt.mime, func(t *testing.T) {
			got := extensionForMIME(tt.mime)
			if got != tt.ext {
				t.Errorf("extensionForMIME(%q) = %q, want %q", tt.mime, got, tt.ext)
			}
		})
	}
}

func TestContainsTraversal(t *testing.T) {
	tests := []struct {
		path   string
		expect bool
	}{
		{"assets", false},
		{"img/screenshots", false},
		{".hidden/assets", false},
		{"..", true},
		{"../etc", true},
		{"assets/../../../etc", true},
		{"foo/../../bar", true},
	}
	for _, tt := range tests {
		t.Run(tt.path, func(t *testing.T) {
			got := containsTraversal(tt.path)
			if got != tt.expect {
				t.Errorf("containsTraversal(%q) = %v, want %v", tt.path, got, tt.expect)
			}
		})
	}
}

func TestHandleImageSaved_InsertsMarkdownRef(t *testing.T) {
	m := newImageTestEditor(terminal.TermCaps{})
	m = m.SetContent("/tmp/note.md", []byte("hello world"))
	m = m.SetSize(80, 24)

	now := time.Now()
	m, _ = m.handleImageSaved("assets/20250315-abc123.png", now)

	content := m.Content()
	if !strings.Contains(content, "![image](assets/20250315-abc123.png)") {
		t.Errorf("expected markdown reference in content, got %q", content)
	}
}
