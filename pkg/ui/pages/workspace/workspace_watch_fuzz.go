//go:build fuzzing

package workspace

import (
	"context"
	"fmt"

	tea "charm.land/bubbletea/v2"
)

// watchDirCmd is a no-op under the fuzzing build tag. fsnotify events are
// injected deterministically via FuzzDirChangedMsg / FuzzFileWatchReadErrorMsg
// so the corpus is shrinkable and reproducible.
func watchDirCmd(ctx context.Context, dir string) tea.Cmd { return nil }

// FuzzDirChangedMsg returns a tea.Msg equivalent to dirChangedMsg{}, the
// internal message produced when fsnotify detects a dir change. The driver
// injects this to simulate a filesystem notification without a real watcher.
func FuzzDirChangedMsg() tea.Msg { return dirChangedMsg{} }

// FuzzFileWatchReadErrorMsg returns a tea.Msg equivalent to fileWatchReadError,
// the internal message produced when fsnotify detects a write but the re-read
// fails. Used by the driver to simulate watcher read errors.
func FuzzFileWatchReadErrorMsg(path string, err error) tea.Msg {
	if err == nil {
		err = fmt.Errorf("simulated read error")
	}
	return fileWatchReadError{path: path, err: err}
}
