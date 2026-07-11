//go:build darwin

package vfs

import (
	"errors"
	"io/fs"
	"path/filepath"
	"testing"
)

// TestDiskExchange_SupportedOnDarwin asserts Disk{}.Exchange does NOT return
// ErrUnsupported on the dev machine — the syscall path (renamex_np +
// RENAME_SWAP) must actually be wired up on darwin, not silently degrade to
// the portable stub.
func TestDiskExchange_SupportedOnDarwin(t *testing.T) {
	dir := t.TempDir()
	a := filepath.Join(dir, "a.md")
	b := filepath.Join(dir, "b.md")
	if err := (Disk{}).WriteFile(a, []byte("a"), 0o644); err != nil {
		t.Fatalf("WriteFile a: %v", err)
	}
	if err := (Disk{}).WriteFile(b, []byte("b"), 0o644); err != nil {
		t.Fatalf("WriteFile b: %v", err)
	}
	err := (Disk{}).Exchange(a, b)
	if errors.Is(err, ErrUnsupported) {
		t.Fatal("Disk.Exchange returned ErrUnsupported on darwin — syscall wiring is broken")
	}
	if err != nil {
		t.Fatalf("Disk.Exchange: %v", err)
	}
}

// TestDiskRenameExcl_PublishesDurablyAndNoClobber exercises the create-path
// publish primitive end to end (§1.4.1): WriteFile a sibling temp (durable,
// not atomic), RenameExcl it onto a not-yet-existing target (atomic publish,
// parent dir fsynced), then confirm the temp is gone (renamed away, not
// copied) and a second RenameExcl of a fresh temp onto the now-occupied
// target refuses with fs.ErrExist rather than clobbering — no window where
// an already-published file can be silently overwritten by this primitive.
func TestDiskRenameExcl_PublishesDurablyAndNoClobber(t *testing.T) {
	dir := t.TempDir()
	target := filepath.Join(dir, "note.md")
	temp := filepath.Join(dir, ".note.md.tmp")

	if err := (Disk{}).WriteFile(temp, []byte("first"), 0o644); err != nil {
		t.Fatalf("WriteFile temp: %v", err)
	}
	if err := (Disk{}).RenameExcl(temp, target); err != nil {
		t.Fatalf("RenameExcl (create): %v", err)
	}
	got, err := (Disk{}).ReadFile(target)
	if err != nil || string(got) != "first" {
		t.Fatalf("ReadFile target after RenameExcl: got %q, err %v", got, err)
	}
	if _, err := (Disk{}).Stat(temp); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("temp %q still exists after RenameExcl (should have been renamed away): stat err=%v", temp, err)
	}

	// A second publish attempt onto the now-occupied target must refuse, not
	// clobber the already-published bytes.
	temp2 := filepath.Join(dir, ".note.md.tmp2")
	if err := (Disk{}).WriteFile(temp2, []byte("second"), 0o644); err != nil {
		t.Fatalf("WriteFile temp2: %v", err)
	}
	err = (Disk{}).RenameExcl(temp2, target)
	if !errors.Is(err, fs.ErrExist) {
		t.Fatalf("RenameExcl onto occupied target: got err=%v, want fs.ErrExist", err)
	}
	got, err = (Disk{}).ReadFile(target)
	if err != nil || string(got) != "first" {
		t.Fatalf("target clobbered by refused RenameExcl: got %q, err %v", got, err)
	}
}
