//go:build darwin

package vfs

import (
	"fmt"
	"os/exec"
	"path/filepath"
)

// S1: /usr/bin/trash parses a "-"-leading argument as a flag (e.g. a file
// literally named "-s" is silently swallowed as the stopOnError flag —
// verified empirically: exit 0, no error, file left untouched, no trash
// entry created). A relative path whose basename happens to start with "-"
// reaches this function unmodified from the filetree/rename/undo paths, so
// resolve to an absolute path FIRST — every real absolute path starts with
// "/", never "-", closing the flag-injection vector regardless of the
// caller's argument.
func (Disk) Trash(path string) error {
	if !filepath.IsAbs(path) {
		abs, err := filepath.Abs(path)
		if err != nil {
			return fmt.Errorf("trash %q: resolve absolute path: %w", path, err)
		}
		path = abs
	}
	if err := exec.Command("/usr/bin/trash", path).Run(); err != nil {
		return fmt.Errorf("run /usr/bin/trash %q: %w", path, err)
	}
	return nil
}
