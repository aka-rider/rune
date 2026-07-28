package workspace

import (
	"context"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/fsnotify/fsnotify"
)

// Watcher abstracts directory-change notification so tests and the session
// fuzzer can supply a deterministic double instead of a real fsnotify watcher
// blocking on a live OS channel with no timeout of its own. Mirrors vfs.FS:
// a small role interface, a Disk-analogous real implementation
// (FSNotifyWatcher), and a production-defined fake (NoopWatcher) shared by
// tests and the fuzzer. Model stores it as a nil-defaulting field, injected
// via WithWatcher and read only through watcher().
type Watcher interface {
	// WatchDir returns a tea.Cmd that watches dir for changes, yielding
	// dirChangedMsg or fileChangedMsg on a qualifying fsnotify event, or nil
	// once ctx is canceled.
	WatchDir(ctx context.Context, dir string) tea.Cmd
}

// FSNotifyWatcher is the production Watcher, backed by a real fsnotify
// watcher.
type FSNotifyWatcher struct{}

// NoopWatcher is a Watcher that never watches anything: WatchDir returns a
// nil tea.Cmd, so startWatch's re-arm never spawns a goroutine or opens an OS
// watch descriptor. Injected by every test constructor in this package and by
// the session fuzzer — neither depends on a real fsnotify event ever
// arriving, since the watch-triggered handlers are exercised by delivering
// dirChangedMsg/fileChangedMsg directly. This makes a leaked watcher
// goroutine structurally impossible in those paths, rather than merely
// bounded-and-abandoned (see the formerly-TODO'd execFastCmds workaround,
// now removed).
type NoopWatcher struct{}

// WatchDir implements Watcher.
func (NoopWatcher) WatchDir(ctx context.Context, dir string) tea.Cmd { return nil }

// isStructuralEvent reports whether ev is a Create/Remove/Rename for writePath
// specifically — not just any structural event in the watched directory. An
// unrelated file's churn (another app's temp file, a git operation, a new note)
// must not be able to mask a genuine Write to writePath as "explained away."
// Our own atomic save still classifies structural correctly regardless: its
// first observed event (the temp file's Create, chronologically before the
// rename) already latches `structural` via the OUTER classification in
// WatchDir, before this path-scoped check inside the drain loop is ever
// reached.
func isStructuralEvent(ev fsnotify.Event, writePath string) bool {
	if !(ev.Has(fsnotify.Create) || ev.Has(fsnotify.Remove) || ev.Has(fsnotify.Rename)) {
		return false
	}
	return ev.Name == writePath
}

// WatchDir implements Watcher.
func (FSNotifyWatcher) WatchDir(ctx context.Context, dir string) tea.Cmd {
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
