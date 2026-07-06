//go:build !darwin

package vfs

import "fmt"

// Trash is unsupported outside darwin: no portable "move to OS trash"
// primitive is wired up here (Disk delegates to /usr/bin/trash on darwin,
// which has no equivalent on other platforms — a freedesktop.org trash-spec
// implementation for Linux is a future extension, not required). Refusing
// with ErrUnsupported (surfaced to the user via fileTrashCmd's
// FileDeleteErrorMsg) is the safe choice: a Trash call exists specifically
// so a delete is recoverable, so silently falling back to a permanent
// os.Remove would be the wrong default (§0 — never surprise the user with
// data loss they didn't ask for).
func (Disk) Trash(path string) error {
	return fmt.Errorf("trash %q: %w", path, ErrUnsupported)
}
