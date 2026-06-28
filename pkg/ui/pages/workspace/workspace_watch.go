//go:build !fuzzing

package workspace

import (
	"context"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/fsnotify/fsnotify"
)

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
				if event.Has(fsnotify.Create) || event.Has(fsnotify.Remove) || event.Has(fsnotify.Rename) {
					timer := time.NewTimer(50 * time.Millisecond)
				drain:
					for {
						select {
						case <-ctx.Done():
							timer.Stop()
							return nil
						case <-timer.C:
							break drain
						case _, ok := <-watcher.Events:
							if !ok {
								break drain
							}
							timer.Reset(50 * time.Millisecond)
						}
					}
					return dirChangedMsg{}
				}
			case _, ok := <-watcher.Errors:
				if !ok {
					return nil
				}
				return nil // fire-and-forget: watcher error
			}
		}
	}
}
