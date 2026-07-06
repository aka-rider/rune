package workspace

import (
	"context"
	"errors"
	"fmt"
	"io/fs"
	"path/filepath"
	"sort"
	"strings"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/docstate"
	"rune/pkg/ui/components/filetree"
	"rune/pkg/vfs"
)

// ---- Message types (D12: workspace owns the file/disk domain) ----

// FileLoadedMsg is returned when store.Load has read a file and recorded its
// disk-fact observation. Gen is the load generation this read was issued
// under (§ workspace.loadGen); the handler installs the content+identity
// ONLY if Gen still matches the awaited load, so a superseded or
// out-of-order read can never display the wrong document.
type FileLoadedMsg struct {
	Path   string
	Result docstate.LoadResult
	Gen    uint64
}

// FileLoadErrorMsg is returned when a file load fails. Gen mirrors FileLoadedMsg.Gen
// so a stale failure cannot surface an error for a load the user already moved past.
type FileLoadErrorMsg struct {
	Path string
	Err  error
	Gen  uint64
}

// FileSavedMsg is returned when store.Materialize has committed a write.
type FileSavedMsg struct {
	Path      string
	DocID     int64 // the VFS doc materialized (0 if storeless)
	RequestID string
	BindNew   bool // true when this was a first-bind of an untitled doc
	Result    docstate.MatResult
}

// FileSaveErrorMsg is returned when store.Materialize refuses (Conflict or
// Missing) or fails outright (Err). Conflict is a CAS refusal — a fresher
// disk observation was recorded instead (Fresh). Missing means the target
// vanished and the save was not an explicit bindNew — never a silent
// recreate (§1.4.4); the caller routes to the deleted guard instead. The
// three are mutually exclusive discriminants (§1.7).
type FileSaveErrorMsg struct {
	Path      string
	DocID     int64
	RequestID string
	Err       error
	Conflict  bool
	Missing   bool
	Fresh     docstate.Observation // populated when Conflict
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

// SaveIdentity tracks an in-flight save operation (D12, D13). Path/DocID are the
// identity of the file THIS save is writing, captured once at save-start — never
// re-derived from m.view later, since m.view can drift to a different document
// while the save's write is still in flight (mouse-driven tab switch has no
// keyboard-style InFlight gate). savingTarget (workspace_probe.go)
// compares against these, not against m.view's current identity.
type SaveIdentity struct {
	RequestID    string
	SavedContent []byte // snapshot of content at save-start; used as new origContent on ack
	InFlight     bool
	Path         string
	DocID        int64
}

// ---- Cmd factories (D12) ----

// loadFileCmd reads path through the store (identity, recovery, and the
// resulting SyncState in ONE round trip — docstate.Store.Load). gen is the
// load generation this read was issued under; it is stamped onto the result
// so the handler can drop a stale (superseded/out-of-order) read.
//
// store may be nil early in startup (the async StoreReadyMsg has not landed
// yet, e.g. opening files passed on the command line) — falls back to a raw
// disk read with no identity/history/SyncState, exactly like the pre-v4
// loadFileCmd; StoreReadyMsg's handler re-resolves identity for whatever is
// displayed once the store becomes ready (workspace_io_handlers.go).
func loadFileCmd(store *docstate.Store, fsys vfs.FS, ctx context.Context, path string, gen uint64) tea.Cmd {
	return func() tea.Msg {
		select {
		case <-ctx.Done():
			return nil
		default:
		}
		if store == nil {
			data, err := fsys.ReadFile(path)
			if err != nil {
				return FileLoadErrorMsg{Path: path, Err: fmt.Errorf("read %q: %w", path, err), Gen: gen}
			}
			content := string(data)
			return FileLoadedMsg{
				Path:   path,
				Result: docstate.LoadResult{DiskContent: content, Recovered: content},
				Gen:    gen,
			}
		}
		result, err := store.Load(path)
		if err != nil {
			return FileLoadErrorMsg{Path: path, Err: fmt.Errorf("load %q: %w", path, err), Gen: gen}
		}
		return FileLoadedMsg{Path: path, Result: result, Gen: gen}
	}
}

// materializeStoreCmd is the single VFS→disk write primitive (Fix 4, WP5):
// store.Materialize performs the full CAS-write protocol (Part III) —
// unconditional pre-write hash, atomic swap, displaced-bytes re-check, one
// commit tx — so there is no separate baseline/backstop plumbing here
// anymore. expect is the CAS expectation (typically SavedObs captured
// synchronously at save-start); seq is the journal position content
// corresponds to, ALSO captured at save-start (co-atomic with content —
// §1.4.2/§1.4.8; see docstate.Store.Materialize's doc comment).
func materializeStoreCmd(store *docstate.Store, docID int64, path, content string, expect docstate.ObsID, seq int64, requestID string, bindNew bool) tea.Cmd {
	return func() tea.Msg {
		result, err := store.Materialize(docID, path, content, expect, seq, bindNew)
		if err != nil {
			return FileSaveErrorMsg{
				Path: path, DocID: docID, RequestID: requestID,
				Err: fmt.Errorf("materialize %q: %w", path, err),
			}
		}
		if !result.Committed {
			return FileSaveErrorMsg{
				Path: path, DocID: docID, RequestID: requestID,
				Conflict: !result.Missing, Missing: result.Missing, Fresh: result.Fresh,
			}
		}
		return FileSavedMsg{Path: path, DocID: docID, RequestID: requestID, BindNew: bindNew, Result: result}
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

// fileRenameCmd moves a file on disk. It refuses to clobber an existing
// target (F9): vfs.RenameExcl is atomic and no-clobber (fs.ErrExist if
// newPath already exists), closing the Stat-then-Rename TOCTOU window a
// concurrent creator could otherwise win (G1). Where RenameExcl itself is
// unsupported on this platform/filesystem (vfs.ErrUnsupported — no hardlink
// support), falls back to the previous Stat-guarded Rename with the residual
// (documented, narrow) race window.
func fileRenameCmd(fsys vfs.FS, oldPath, newPath string) tea.Cmd {
	return func() tea.Msg {
		err := fsys.RenameExcl(oldPath, newPath)
		switch {
		case err == nil:
			return FileRenamedMsg{OldPath: oldPath, NewPath: newPath}
		case errors.Is(err, fs.ErrExist):
			return FileRenameErrorMsg{
				Err: fmt.Errorf("rename %q → %q: target already exists", oldPath, newPath),
			}
		case !errors.Is(err, vfs.ErrUnsupported):
			return FileRenameErrorMsg{
				Err: fmt.Errorf("rename %q → %q: %w", oldPath, newPath, err),
			}
		}
		// Portable fallback: RenameExcl unsupported here — degrade to the
		// Stat-then-Rename check. A creator racing between the Stat and the
		// Rename can still win; RenameExcl's atomic no-clobber guarantee
		// only holds where the platform actually supports it.
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
