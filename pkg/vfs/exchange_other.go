//go:build !darwin

package vfs

import (
	"errors"
	"fmt"
	"io/fs"
	"os"
	"syscall"
)

// Exchange is unsupported outside darwin: no portable atomic-swap syscall is
// wired up here (Linux renameat2(RENAME_EXCHANGE) is a future extension, not
// required — see Part IV of the data-integrity plan). Materialize's documented
// fallback is probe+atomic-write with an unconditional pre-write hash bounding
// the residual race window to microseconds.
func (Disk) Exchange(a, b string) error {
	return fmt.Errorf("exchange %q <-> %q: %w", a, b, ErrUnsupported)
}

// RenameExcl falls back to the portable Link-then-Remove no-clobber pattern:
// os.Link fails atomically with fs.ErrExist if newPath already exists, and
// only then is oldPath unlinked. A filesystem that doesn't support hard links
// at all (EPERM/ENOTSUP/EXDEV-class — e.g. some network or FAT-family
// filesystems, or a rename crossing a volume boundary) degrades to
// ErrUnsupported rather than a hard failure, so callers (F9's fileRenameCmd)
// can fall back deliberately to a documented, narrower-guarantee path instead
// of refusing the operation outright.
func (Disk) RenameExcl(oldPath, newPath string) error {
	if err := os.Link(oldPath, newPath); err != nil {
		if errors.Is(err, fs.ErrExist) {
			return fmt.Errorf("renameexcl %q -> %q: %w", oldPath, newPath, fs.ErrExist)
		}
		if isLinkUnsupported(err) {
			return fmt.Errorf("renameexcl %q -> %q: %w", oldPath, newPath, ErrUnsupported)
		}
		return fmt.Errorf("renameexcl %q -> %q: %w", oldPath, newPath, err)
	}
	if err := os.Remove(oldPath); err != nil {
		return fmt.Errorf("renameexcl %q -> %q: remove old: %w", oldPath, newPath, err)
	}
	return nil
}

// isLinkUnsupported reports whether err from os.Link indicates the
// filesystem doesn't support hard links at all (rather than a real,
// unrelated failure): permission-denied-for-links, not-supported, or a
// cross-device rename target — every one of which means "this primitive
// can't work here," not "something went wrong."
func isLinkUnsupported(err error) bool {
	return errors.Is(err, syscall.EPERM) ||
		errors.Is(err, syscall.ENOTSUP) ||
		errors.Is(err, syscall.EXDEV)
}
