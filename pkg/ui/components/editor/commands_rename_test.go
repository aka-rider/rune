package editor

import (
	"os"
	"path/filepath"
	"testing"
)

func TestFileRenameCmd_Success(t *testing.T) {
	dir := t.TempDir()
	oldPath := filepath.Join(dir, "a.md")
	if err := os.WriteFile(oldPath, []byte("content"), 0644); err != nil {
		t.Fatalf("setup: %v", err)
	}

	cmd := FileRenameCmd(oldPath, "b")
	result := cmd()

	msg, ok := result.(FileRenamedMsg)
	if !ok {
		t.Fatalf("expected FileRenamedMsg, got %T: %v", result, result)
	}
	if msg.OldPath != oldPath {
		t.Errorf("OldPath: got %q, want %q", msg.OldPath, oldPath)
	}
	expectedNew := filepath.Join(dir, "b.md")
	if msg.NewPath != expectedNew {
		t.Errorf("NewPath: got %q, want %q", msg.NewPath, expectedNew)
	}
	if _, err := os.Stat(expectedNew); err != nil {
		t.Errorf("new file should exist: %v", err)
	}
}

func TestFileRenameCmd_PreservesExtension(t *testing.T) {
	dir := t.TempDir()
	oldPath := filepath.Join(dir, "note.md")
	if err := os.WriteFile(oldPath, []byte(""), 0644); err != nil {
		t.Fatalf("setup: %v", err)
	}

	cmd := FileRenameCmd(oldPath, "renamed")
	result := cmd()

	msg, ok := result.(FileRenamedMsg)
	if !ok {
		t.Fatalf("expected FileRenamedMsg, got %T", result)
	}
	if filepath.Ext(msg.NewPath) != ".md" {
		t.Errorf("expected .md extension, got %q", filepath.Ext(msg.NewPath))
	}
}

func TestFileRenameCmd_NonExistent(t *testing.T) {
	cmd := FileRenameCmd("/nonexistent/path/that/does/not/exist.md", "new")
	result := cmd()

	msg, ok := result.(FileRenameErrorMsg)
	if !ok {
		t.Fatalf("expected FileRenameErrorMsg, got %T", result)
	}
	if msg.Err == nil {
		t.Error("expected non-nil error")
	}
}
