//go:build !fuzzing

package workspace

import (
	"context"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/fsnotify/fsnotify"
)

// isStructuralEvent reports whether ev is a Create/Remove/Rename for writePath
// specifically — not just any structural event in the watched directory. An
// unrelated file's churn (another app's temp file, a git operation, a new note)
// must not be able to mask a genuine Write to writePath as "explained away."
// Our own atomic save still classifies structural correctly regardless: its
// first observed event (the temp file's Create, chronologically before the
// rename) already latches `structural` via the OUTER classification in
// watchDirCmd, before this path-scoped check inside the drain loop is ever
// reached.
func isStructuralEvent(ev fsnotify.Event, writePath string) bool {
	if !(ev.Has(fsnotify.Create) || ev.Has(fsnotify.Remove) || ev.Has(fsnotify.Rename)) {
		return false
	}
	return ev.Name == writePath
}

func watchDirCmd(ctx context.Context, dir string) tea.Cmd {
	return func() tea.Msg {
		watcher, err := fsnotify.NewWatcher()
		if err != nil {
			return nil // fire-and-forget: watcher creation failed
		}
		defer watcher.Close()

		if err := watcher.Add(dir); err != nil {
			return nil // fire-and-forget: dir may have been removed
		}

		for {
			select {
			case <-ctx.Done():
				return nil
			case event, ok := <-watcher.Events:
				if !ok {
					return nil
				}
				structural := event.Has(fsnotify.Create) || event.Has(fsnotify.Remove) || event.Has(fsnotify.Rename)
				// In-place Write (BUG1): an external editor truncating/writing the
				// same inode. Our own atomic save lands via temp→rename → Create, so
				// it surfaces as `structural`, never a Write on the target.
				write := !structural && event.Has(fsnotify.Write)
				if !structural && !write {
					continue
				}
				writePath := event.Name
				timer := time.NewTimer(50 * time.Millisecond)
			drain:
				for {
					select {
					case <-ctx.Done():
						timer.Stop()
						return nil
					case <-timer.C:
						break drain
					case ev, ok := <-watcher.Events:
						if !ok {
							break drain
						}
						// Only a structural event for writePath itself upgrades the
						// batch to a dir change — an unrelated file's Create/Remove/
						// Rename elsewhere in the directory must not mask a genuine
						// Write to the open file (see isStructuralEvent).
						if isStructuralEvent(ev, writePath) {
							structural = true
						}
						timer.Reset(50 * time.Millisecond)
					}
				}
				if structural {
					return dirChangedMsg{}
				}
				return fileChangedMsg{path: writePath}
			case _, ok := <-watcher.Errors:
				if !ok {
					return nil
				}
				return nil // fire-and-forget: watcher error
			}
		}
	}
}
