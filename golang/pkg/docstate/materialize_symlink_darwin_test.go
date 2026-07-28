//go:build darwin

package docstate

import (
	"os"
	"path/filepath"
	"testing"

	"rune/pkg/vfs"
)

// TestMaterialize_Symlink_WritesResolvedTargetLinkSurvives: on a real
// tempdir with Disk{}, saving through a symlink must write the RESOLVED
// target's bytes (never replace the link itself), and the link must still
// point at the target afterward.
func TestMaterialize_Symlink_WritesResolvedTargetLinkSurvives(t *testing.T) {
	dir := t.TempDir()
	target := filepath.Join(dir, "real.md")
	if err := os.WriteFile(target, []byte("original"), 0o644); err != nil {
		t.Fatalf("seed target: %v", err)
	}
	link := filepath.Join(dir, "link.md")
	if err := os.Symlink(target, link); err != nil {
		t.Fatalf("Symlink: %v", err)
	}

	s := NewTestStore(t)
	s.UseFS(vfs.Disk{})

	loaded, err := s.Load(link)
	if err != nil {
		t.Fatalf("Load(link): %v", err)
	}
	expect, _, err := s.SavedObs(loaded.DocID)
	if err != nil {
		t.Fatalf("SavedObs: %v", err)
	}

	result, err := s.Materialize(loaded.DocID, link, "written through the symlink", expect.ID, 0, false)
	if err != nil {
		t.Fatalf("Materialize: %v", err)
	}
	if !result.Committed {
		t.Fatal("Materialize: want Committed=true")
	}

	// The link itself must survive as a symlink pointing at target.
	fi, err := os.Lstat(link)
	if err != nil {
		t.Fatalf("Lstat(link): %v", err)
	}
	if fi.Mode()&os.ModeSymlink == 0 {
		t.Fatal("the symlink was replaced by a regular file — Materialize must write through it, not over it")
	}
	resolved, err := os.Readlink(link)
	if err != nil {
		t.Fatalf("Readlink: %v", err)
	}
	if resolved != target {
		t.Fatalf("link now points at %q, want %q", resolved, target)
	}

	// The TARGET holds the new content.
	got, err := os.ReadFile(target)
	if err != nil {
		t.Fatalf("ReadFile(target): %v", err)
	}
	if string(got) != "written through the symlink" {
		t.Fatalf("target content = %q, want %q", got, "written through the symlink")
	}
}

// TestMaterialize_RenameExclCreate_NoClobberOntoExistingPath: RenameExcl's
// create path refuses to clobber an existing target, on real disk.
func TestMaterialize_RenameExclCreate_NoClobberOntoExistingPath(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "existing.md")
	if err := os.WriteFile(path, []byte("already here"), 0o644); err != nil {
		t.Fatalf("seed existing file: %v", err)
	}

	s := NewTestStore(t)
	s.UseFS(vfs.Disk{})
	ref, err := s.CreateScratch("Untitled")
	if err != nil {
		t.Fatalf("CreateScratch: %v", err)
	}
	if err := s.Bind(ref.ID, path); err != nil {
		t.Fatalf("Bind: %v", err)
	}

	// documents.path now claims `path`, but the row's inode/device were
	// stamped from the pre-existing file at Bind time — simulate the
	// "bind-new" gap by deleting the file out from under it so Materialize's
	// create path (target doesn't exist right now) actually engages
	// RenameExcl, and something else races back in before the rename.
	if err := os.Remove(path); err != nil {
		t.Fatalf("remove to re-arm create path: %v", err)
	}

	raced := false
	hook := &hookFS{FS: vfs.Disk{}, beforeRenameExcl: func(real vfs.FS) {
		if raced {
			return
		}
		raced = true
		if err := os.WriteFile(path, []byte("concurrent creator"), 0o644); err != nil {
			t.Fatalf("race hook write: %v", err)
		}
	}}
	s.UseFS(hook)

	result, err := s.Materialize(ref.ID, path, "our content", 0, 0, true)
	if err != nil {
		t.Fatalf("Materialize: %v", err)
	}
	if result.Committed {
		t.Fatal("Materialize: want Committed=false (no-clobber refusal), got true")
	}
	got, err := os.ReadFile(path)
	if err != nil || string(got) != "concurrent creator" {
		t.Fatalf("concurrent creator's bytes clobbered: got %q, err %v", got, err)
	}
}
