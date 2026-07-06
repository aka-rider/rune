//go:build darwin

package vfs

import (
	"errors"
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
