package workspace

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/atomicfile"
	"rune/pkg/ui/components/filetree"
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

// fileCreatedMsg is the result of createFileCmd.
type fileCreatedMsg struct {
	path    string
	content string
	err     error
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

// saveFileCmd atomically writes content to path and returns FileSavedMsg or
// FileSaveErrorMsg. Writes go via temp→Sync→Rename so a crash mid-write never
// produces a partial or empty file at the target path (CLAUDE.md §1.4.1).
func saveFileCmd(path, content, requestID string) tea.Cmd {
	capturedBytes := []byte(content)
	return func() tea.Msg {
		if err := atomicfile.Write(path, capturedBytes); err != nil {
			return FileSaveErrorMsg{
				Path:      path,
				RequestID: requestID,
				Err:       err,
			}
		}
		return FileSavedMsg{
			Path:         path,
			RequestID:    requestID,
			SavedContent: capturedBytes,
		}
	}
}

// fileRenameCmd renames a file on disk.
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

// createFileCmd creates a new file at path with the given content. It refuses
// to clobber an existing file (CLAUDE.md §1.4.1) and writes atomically.
func createFileCmd(path, content string) tea.Cmd {
	capturedBytes := []byte(content)
	return func() tea.Msg {
		dir := filepath.Dir(path)
		if err := os.MkdirAll(dir, 0755); err != nil {
			return fileCreatedMsg{path: path, err: fmt.Errorf("mkdir %q: %w", dir, err)}
		}
		// Refuse to clobber: naming an untitled buffer over an existing file would
		// silently truncate it (Catastrophic, rung 1 — CLAUDE.md §1.4.1).
		if _, err := os.Stat(path); err == nil {
			return fileCreatedMsg{path: path, err: fmt.Errorf("create %q: file already exists", path)}
		}
		if err := atomicfile.Write(path, capturedBytes); err != nil {
			return fileCreatedMsg{path: path, err: fmt.Errorf("create %q: %w", path, err)}
		}
		return fileCreatedMsg{path: path, content: content}
	}
}

// loadDirCmd loads directory entries for the given dir.
func loadDirCmd(dir string) tea.Cmd {
	return func() tea.Msg {
		entries, err := readDirEntries(dir)
		if err != nil {
			return ErrMsg{Err: err}
		}
		return filetree.DirLoadedMsg{Root: dir, Entries: entries}
	}
}

// reloadDirCmd reloads directory entries after a watched change.
func reloadDirCmd(dir string) tea.Cmd {
	return func() tea.Msg {
		entries, err := readDirEntries(dir)
		if err != nil {
			return ErrMsg{Err: err}
		}
		return filetree.DirReloadedMsg{Root: dir, Entries: entries}
	}
}

func readDirEntries(dir string) ([]filetree.Entry, error) {
	des, err := os.ReadDir(dir)
	if err != nil {
		return nil, fmt.Errorf("load dir %q: %w", dir, err)
	}
	entries := make([]filetree.Entry, 0, len(des)+1)
	if dir == "/" {
		entries = append(entries, filetree.Entry{Name: "/", Path: "/", IsDir: true})
	} else {
		entries = append(entries, filetree.Entry{Name: "..", Path: filepath.Dir(dir), IsDir: true})
	}
	for _, de := range des {
		entries = append(entries, filetree.Entry{
			Name:  de.Name(),
			Path:  filepath.Join(dir, de.Name()),
			IsDir: de.IsDir(),
		})
	}
	sort.Slice(entries, func(i, j int) bool {
		a, b := entries[i], entries[j]
		if a.Name == ".." {
			return true
		}
		if b.Name == ".." {
			return false
		}
		if a.IsDir != b.IsDir {
			return a.IsDir
		}
		return strings.ToLower(a.Name) < strings.ToLower(b.Name)
	})
	return entries, nil
}

func (m Model) startWatch(dir string) (Model, tea.Cmd) {
	if m.cancelWatch != nil {
		m.cancelWatch()
	}
	ctx, cancel := context.WithCancel(context.Background())
	m.cancelWatch = cancel
	m.watchedDir = dir
	return m, watchDirCmd(ctx, dir)
}

const invalidFileNameChars = "/\\:*?\"<>|\x00"

func validateFileName(name string) error {
	if name == "" {
		return fmt.Errorf("name must not be empty")
	}
	for _, r := range name {
		for _, bad := range invalidFileNameChars {
			if r == bad {
				return fmt.Errorf("name contains invalid character %q", r)
			}
		}
		if r < 32 {
			return fmt.Errorf("name contains control character")
		}
	}
	return nil
}
