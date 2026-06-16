package workspace

import (
	"context"
	"fmt"
	"os"

	tea "charm.land/bubbletea/v2"
)

// ---- Message types (D12: workspace owns the file/disk domain) ----

// FileLoadedMsg is returned when a file has been read from disk.
type FileLoadedMsg struct {
	Path    string
	Content []byte
}

// FileLoadErrorMsg is returned when a file load fails.
type FileLoadErrorMsg struct {
	Path string
	Err  error
}

// FileSavedMsg is returned when a file has been written to disk.
type FileSavedMsg struct {
	Path         string
	RequestID    string
	SavedContent []byte // exact bytes written — used as new origContent (D13 Finding A)
}

// FileSaveErrorMsg is returned when a file save fails.
type FileSaveErrorMsg struct {
	Path      string
	RequestID string
	Err       error
}

// FileRenamedMsg is returned after a successful file rename.
type FileRenamedMsg struct {
	OldPath string
	NewPath string
}

// FileRenameErrorMsg is returned when a file rename fails.
type FileRenameErrorMsg struct {
	Err error
}

// SaveIdentity tracks an in-flight save operation (D12, D13).
type SaveIdentity struct {
	RequestID    string
	SavedContent []byte // snapshot of content at save-start; used as new origContent on ack
	InFlight     bool
}

// ---- Cmd factories (D12) ----

// loadFileCmd reads a file from disk. Context-cancellable for rapid tab switching.
func loadFileCmd(ctx context.Context, path string) tea.Cmd {
	return func() tea.Msg {
		select {
		case <-ctx.Done():
			return nil
		default:
		}
		b, err := os.ReadFile(path)
		if err != nil {
			return FileLoadErrorMsg{Path: path, Err: fmt.Errorf("read %q: %w", path, err)}
		}
		return FileLoadedMsg{Path: path, Content: b}
	}
}

// saveFileCmd writes content to path and returns FileSavedMsg or FileSaveErrorMsg.
func saveFileCmd(path, content, requestID string) tea.Cmd {
	capturedBytes := []byte(content)
	return func() tea.Msg {
		if err := os.WriteFile(path, capturedBytes, 0o644); err != nil {
			return FileSaveErrorMsg{
				Path:      path,
				RequestID: requestID,
				Err:       fmt.Errorf("write %q: %w", path, err),
			}
		}
		return FileSavedMsg{
			Path:         path,
			RequestID:    requestID,
			SavedContent: capturedBytes,
		}
	}
}

// fileRenameCmd renames a file from oldPath to a new name in the same directory.
// newName must be just the base name (no directory component).
func fileRenameCmd(oldPath, newPath string) tea.Cmd {
	return func() tea.Msg {
		if err := os.Rename(oldPath, newPath); err != nil {
			return FileRenameErrorMsg{
				Err: fmt.Errorf("rename %q → %q: %w", oldPath, newPath, err),
			}
		}
		return FileRenamedMsg{OldPath: oldPath, NewPath: newPath}
	}
}
