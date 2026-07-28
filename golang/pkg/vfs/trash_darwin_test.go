//go:build darwin

package vfs_test

import (
	"os"
	"os/exec"
	"path/filepath"
	"testing"

	"rune/pkg/vfs"
)

// TestTrash_RelativeDashLeadingNameIsNotMisparsedAsFlag is S1's regression
// test: /usr/bin/trash parses a "-"-leading path argument as a flag — a file
// literally named "-s" passed as a RELATIVE path is silently swallowed as
// the "stopOnError" flag (verified empirically: exit 0, no error, no trash
// entry, file left completely untouched on disk). Disk.Trash must resolve to
// an absolute path before exec — every real absolute path starts with "/",
// never "-", so a dash-leading basename can never be misread as a flag.
func TestTrash_RelativeDashLeadingNameIsNotMisparsedAsFlag(t *testing.T) {
	if _, err := exec.LookPath("/usr/bin/trash"); err != nil {
		t.Skip("/usr/bin/trash not available on this machine")
	}

	dir := t.TempDir()
	const name = "-s" // a flag /usr/bin/trash recognizes when passed unabsolutized
	path := filepath.Join(dir, name)
	if err := os.WriteFile(path, []byte("x"), 0o644); err != nil {
		t.Fatalf("seed file: %v", err)
	}

	cwd, err := os.Getwd()
	if err != nil {
		t.Fatalf("Getwd: %v", err)
	}
	if err := os.Chdir(dir); err != nil {
		t.Fatalf("Chdir: %v", err)
	}
	defer func() { _ = os.Chdir(cwd) }()

	if err := (vfs.Disk{}).Trash(name); err != nil {
		t.Fatalf("Trash(%q): %v", name, err)
	}

	if _, statErr := os.Stat(path); !os.IsNotExist(statErr) {
		t.Fatalf("Trash(%q) did not remove the file — /usr/bin/trash likely misparsed the relative dash-leading path as a flag (stat err=%v)", name, statErr)
	}
}
