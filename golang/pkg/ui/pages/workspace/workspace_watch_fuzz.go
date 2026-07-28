package workspace

import (
	"fmt"

	tea "charm.land/bubbletea/v2"
)

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

// FuzzFileChangedMsg returns a tea.Msg equivalent to fileChangedMsg, the internal
// message produced when fsnotify observes an in-place Write to a file in the
// watched dir (BUG1). The driver injects this to simulate an external in-place
// edit without a real watcher.
func FuzzFileChangedMsg(path string) tea.Msg { return fileChangedMsg{path: path} }
