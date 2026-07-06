//go:build darwin

package vfs

import (
	"fmt"

	"golang.org/x/sys/unix"
)

// Exchange atomically swaps the contents of a and b via renamex_np(RENAME_SWAP)
// — a single kernel operation, so neither path is ever unlinked (§Part III I1:
// capture-before-discard is physically true, not merely policy). Both paths
// must already exist and be on the same volume. The parent directory of a is
// fsynced afterward for the same durability guarantee atomicfile.Write gives
// an ordinary save (§1.4.1) — the swap itself must survive a crash.
func (Disk) Exchange(a, b string) error {
	if err := unix.RenamexNp(a, b, unix.RENAME_SWAP); err != nil {
		return fmt.Errorf("exchange %q <-> %q: %w", a, b, err)
	}
	if err := fsyncDir(a); err != nil {
		return fmt.Errorf("exchange %q <-> %q: fsync parent: %w", a, b, err)
	}
	return nil
}

// RenameExcl atomically renames oldPath to newPath via renamex_np(RENAME_EXCL),
// failing with an error wrapping fs.ErrExist if newPath already exists.
func (Disk) RenameExcl(oldPath, newPath string) error {
	if err := unix.RenamexNp(oldPath, newPath, unix.RENAME_EXCL); err != nil {
		return fmt.Errorf("renameexcl %q -> %q: %w", oldPath, newPath, err)
	}
	return nil
}
