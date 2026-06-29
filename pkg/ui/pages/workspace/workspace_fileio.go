package workspace

import (
	"context"
	"errors"
	"fmt"
	"io/fs"
	"path/filepath"
	"sort"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/components/filetree"
	"rune/pkg/vfs"
)

// ---- Message types (D12: workspace owns the file/disk domain) ----

// FileLoadedMsg is returned when a file has been read from disk. Gen is the load
// generation this read was issued under (§ workspace.loadGen); the handler installs
// the content+identity ONLY if Gen still matches the awaited load, so a superseded
// or out-of-order read can never display the wrong document.
type FileLoadedMsg struct {
	Path     string
	Content  []byte
	Baseline diskBaseline // fingerprint at read time (§1.4.7 external-change guard)
	Gen      uint64       // load generation; stale (≠ current) results are dropped
}

// diskBaseline fingerprints a file as it was last read or written, so a later
// overwrite can detect that another process changed it underneath us (§1.4.7).
type diskBaseline struct {
	size    int64
	modTime time.Time
	valid   bool
}

// baselineOf stats path through the shim and returns its current fingerprint.
// An unreadable file yields an invalid (zero) baseline.
func baselineOf(fsys vfs.FS, path string) diskBaseline {
	info, err := fsys.Stat(path)
	if err != nil {
		return diskBaseline{}
	}
	return diskBaseline{size: info.Size(), modTime: info.ModTime(), valid: true}
}

// divergedFrom reports whether the file at path differs from this baseline. A
// missing file is NOT divergence (recreating it cannot clobber anything); an
// unreadable file or a size/mtime mismatch is. An invalid baseline never
// diverges (we have nothing to compare against).
func (b diskBaseline) divergedFrom(fsys vfs.FS, path string) bool {
	if !b.valid {
		return false
	}
	info, err := fsys.Stat(path)
	if err != nil {
		return !errors.Is(err, fs.ErrNotExist)
	}
	return info.Size() != b.size || !info.ModTime().Equal(b.modTime)
}

// FileLoadErrorMsg is returned when a file load fails. Gen mirrors FileLoadedMsg.Gen
// so a stale failure cannot surface an error for a load the user already moved past.
type FileLoadErrorMsg struct {
	Path string
	Err  error
	Gen  uint64
}

// FileSavedMsg is returned when a file has been materialized to disk.
type FileSavedMsg struct {
	Path         string
	DocID        int64 // the VFS doc materialized (0 if storeless)
	RequestID    string
	SavedContent []byte       // exact bytes written — used as new origContent (D13 Finding A)
	SavedSeq     int64        // journal position the written bytes reflect, captured at save-start (§1.4.2)
	BindNew      bool         // true when this was a first-bind of an untitled doc
	Baseline     diskBaseline // fingerprint of the file just written (§1.4.7)
}

// FileSaveErrorMsg is returned when a file materialize fails.
type FileSaveErrorMsg struct {
	Path      string
	DocID     int64
	RequestID string
	Err       error
	Conflict  bool // refused to clobber: file exists (bind-new) or changed (overwrite)
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

// FileDeletedMsg is returned after path was successfully moved to trash.
type FileDeletedMsg struct{ Path string }

// FileDeleteErrorMsg is returned when the trash operation fails.
type FileDeleteErrorMsg struct {
	Path string
	Err  error
}

// SaveIdentity tracks an in-flight save operation (D12, D13).
type SaveIdentity struct {
	RequestID    string
	SavedContent []byte // snapshot of content at save-start; used as new origContent on ack
	InFlight     bool
}

// ---- Cmd factories (D12) ----

// loadFileCmd reads a file through the shim. gen is the load generation this read
// was issued under; it is stamped onto the result so the handler can drop a stale
// (superseded / out-of-order) read. fsys is captured into the closure (§6.2), never
// read from the Model.
func loadFileCmd(fsys vfs.FS, ctx context.Context, path string, gen uint64) tea.Cmd {
	return func() tea.Msg {
		select {
		case <-ctx.Done():
			return nil
		default:
		}
		b, err := fsys.ReadFile(path)
		if err != nil {
			return FileLoadErrorMsg{Path: path, Err: fmt.Errorf("read %q: %w", path, err), Gen: gen}
		}
		return FileLoadedMsg{Path: path, Content: b, Baseline: baselineOf(fsys, path), Gen: gen}
	}
}

// materializeCmd is the single VFS→disk write primitive (Fix 4). It writes the
// document's bytes to disk atomically (temp→Sync→Rename→dir-fsync, CLAUDE.md
// §1.4.1) with two clobber guards:
//
//   - bindNew (first save / naming an untitled / rename to a new name): refuses
//     if the target already exists — naming over a real file would truncate it
//     (Catastrophic, rung 1). The buffer is kept; nothing is bound.
//   - overwrite (⌘S on a bound doc): refuses if the file diverged from baseline
//     since it was opened — another editor/tool changed it (§1.4.7). Never
//     silently wins.
//
// Bytes are written verbatim — no line-ending / trailing-newline / BOM
// normalization (§1.4.5). The returned FileSavedMsg carries the fresh baseline.
func materializeCmd(fsys vfs.FS, docID int64, path, content string, savedSeq int64, requestID string, bindNew bool, baseline diskBaseline) tea.Cmd {
	data := []byte(content)
	return func() tea.Msg {
		if bindNew {
			if _, err := fsys.Stat(path); err == nil {
				return FileSaveErrorMsg{
					Path: path, DocID: docID, RequestID: requestID, Conflict: true,
					Err: fmt.Errorf("materialize %q: file already exists", path),
				}
			} else if !errors.Is(err, fs.ErrNotExist) {
				return FileSaveErrorMsg{
					Path: path, DocID: docID, RequestID: requestID,
					Err: fmt.Errorf("materialize %q: stat target: %w", path, err),
				}
			}
			if dir := filepath.Dir(path); dir != "" {
				if err := fsys.MkdirAll(dir, 0o755); err != nil {
					return FileSaveErrorMsg{
						Path: path, DocID: docID, RequestID: requestID,
						Err: fmt.Errorf("materialize %q: mkdir: %w", path, err),
					}
				}
			}
		} else if baseline.divergedFrom(fsys, path) {
			return FileSaveErrorMsg{
				Path: path, DocID: docID, RequestID: requestID, Conflict: true,
				Err: fmt.Errorf("materialize %q: file changed on disk since it was opened — not overwritten", path),
			}
		}
		if err := fsys.WriteFile(path, data, 0o644); err != nil {
			return FileSaveErrorMsg{
				Path: path, DocID: docID, RequestID: requestID,
				Err: fmt.Errorf("materialize %q: %w", path, err),
			}
		}
		return FileSavedMsg{
			Path: path, DocID: docID, RequestID: requestID,
			SavedContent: data, SavedSeq: savedSeq, BindNew: bindNew, Baseline: baselineOf(fsys, path),
		}
	}
}

// fileTrashCmd moves path to the OS trash via the vfs shim.
func fileTrashCmd(fsys vfs.FS, path string) tea.Cmd {
	return func() tea.Msg {
		if err := fsys.Trash(path); err != nil {
			return FileDeleteErrorMsg{Path: path, Err: fmt.Errorf("trash %q: %w", path, err)}
		}
		return FileDeletedMsg{Path: path}
	}
}

// fileRenameCmd moves a file on disk. It refuses to clobber an existing target
// (os.Rename would silently destroy it — Catastrophic, rung 1).
func fileRenameCmd(fsys vfs.FS, oldPath, newPath string) tea.Cmd {
	return func() tea.Msg {
		if _, err := fsys.Stat(newPath); err == nil {
			return FileRenameErrorMsg{
				Err: fmt.Errorf("rename %q → %q: target already exists", oldPath, newPath),
			}
		} else if !errors.Is(err, fs.ErrNotExist) {
			return FileRenameErrorMsg{
				Err: fmt.Errorf("rename %q → %q: stat target: %w", oldPath, newPath, err),
			}
		}
		if err := fsys.Rename(oldPath, newPath); err != nil {
			return FileRenameErrorMsg{
				Err: fmt.Errorf("rename %q → %q: %w", oldPath, newPath, err),
			}
		}
		return FileRenamedMsg{OldPath: oldPath, NewPath: newPath}
	}
}

// loadDirCmd loads directory entries for the given dir through the shim.
func loadDirCmd(fsys vfs.FS, dir string) tea.Cmd {
	return func() tea.Msg {
		entries, err := readDirEntries(fsys, dir)
		if err != nil {
			return ErrMsg{Err: err}
		}
		return filetree.DirLoadedMsg{Root: dir, Entries: entries}
	}
}

// reloadDirCmd reloads directory entries after a watched change.
func reloadDirCmd(fsys vfs.FS, dir string) tea.Cmd {
	return func() tea.Msg {
		entries, err := readDirEntries(fsys, dir)
		if err != nil {
			return ErrMsg{Err: err}
		}
		return filetree.DirReloadedMsg{Root: dir, Entries: entries}
	}
}

func readDirEntries(fsys vfs.FS, dir string) ([]filetree.Entry, error) {
	des, err := fsys.ReadDir(dir)
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
