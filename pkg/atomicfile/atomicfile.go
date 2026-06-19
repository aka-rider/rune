package atomicfile

import (
	"fmt"
	"os"
	"path/filepath"
)

// Write atomically writes data to target by creating a sibling temp file,
// syncing it to durable storage, and atomically renaming it over the target.
// The temp file lives in the same directory as target so the rename is
// same-filesystem. The parent directory is fsynced afterward (best-effort)
// so the rename itself survives a crash (CLAUDE.md §1.4.1).
func Write(target string, data []byte) error {
	dir := filepath.Dir(target)
	tmp, err := os.CreateTemp(dir, ".rune-write-*.tmp")
	if err != nil {
		return fmt.Errorf("atomic write %q: create temp: %w", target, err)
	}
	tmpPath := tmp.Name()

	committed := false
	defer func() {
		if !committed {
			os.Remove(tmpPath) // fire-and-forget: best-effort cleanup of orphaned temp
		}
	}()

	if _, err := tmp.Write(data); err != nil {
		tmp.Close()
		return fmt.Errorf("atomic write %q: write temp: %w", target, err)
	}
	if err := tmp.Sync(); err != nil {
		tmp.Close()
		return fmt.Errorf("atomic write %q: sync temp: %w", target, err)
	}
	if err := tmp.Close(); err != nil {
		return fmt.Errorf("atomic write %q: close temp: %w", target, err)
	}
	if err := os.Rename(tmpPath, target); err != nil {
		return fmt.Errorf("atomic write %q: rename: %w", target, err)
	}
	committed = true

	// Best-effort: fsync the parent directory so the rename itself survives a crash.
	if d, openErr := os.Open(dir); openErr == nil {
		d.Sync() // fire-and-forget: directory fsync is advisory
		d.Close()
	}
	return nil
}
