package vfs

import (
	"io/fs"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"
)

// Mem is a fully in-memory FS for tests and the session fuzzer. It is safe for
// concurrent use (Cmds run in goroutines). Each path gets a synthetic inode on
// first write, kept stable across Rename so §1.4.6 rename-detection is testable;
// the modification clock advances on every write so the §1.4.7 divergence guard
// is exercisable even when content size is unchanged.
//
// By default a path's inode stays stable across repeat WriteFile calls — unlike
// real Disk, whose atomicfile.Write (temp→rename) churns the inode on EVERY
// save. Most consumers want the stable default (several fuzz harnesses use raw
// WriteFile to mean "an external edit happened," a semantic a global churn
// change would silently redefine); opt into real-Disk-like churn per instance
// with WithChurnInodeOnWrite when a test specifically needs to exercise
// save-induced inode churn (e.g. docstate.Store.Bind regression coverage).
type Mem struct {
	mu                sync.Mutex
	files             map[string]*memFile
	nextIno           uint64
	tick              int64 // monotonic source for ModTime; advances on every write
	churnInodeOnWrite bool
}

type memFile struct {
	data    []byte
	inode   uint64
	device  uint64
	modTick int64
}

// MemOption configures a Mem at construction (mirrors textedit.Option).
type MemOption func(*Mem)

// WithChurnInodeOnWrite makes every WriteFile to an EXISTING path assign a
// fresh synthetic inode, matching vfs.Disk's atomic temp-then-rename semantics
// (§1.4.1), where every save gets a new inode. Off by default — see the Mem
// doc comment. Scoped to WriteFile only: Rename's inode preservation (§1.4.6)
// is unaffected regardless of this option.
func WithChurnInodeOnWrite() MemOption {
	return func(m *Mem) { m.churnInodeOnWrite = true }
}

// NewMem returns an empty in-memory filesystem.
func NewMem(opts ...MemOption) *Mem {
	m := &Mem{files: make(map[string]*memFile), nextIno: 1}
	for _, opt := range opts {
		opt(m)
	}
	return m
}

var _ FS = (*Mem)(nil)

func (m *Mem) ReadFile(name string) ([]byte, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	f := m.files[name]
	if f == nil {
		return nil, &fs.PathError{Op: "open", Path: name, Err: fs.ErrNotExist}
	}
	return append([]byte(nil), f.data...), nil
}

// WriteFile stores a private copy of data, assigning a fresh inode the first
// time a path is written (and, with WithChurnInodeOnWrite, on every
// subsequent overwrite too) and advancing the modification clock every time.
func (m *Mem) WriteFile(name string, data []byte, _ fs.FileMode) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	f := m.files[name]
	if f == nil {
		f = &memFile{inode: m.nextIno, device: 1}
		m.nextIno++
		m.files[name] = f
	} else if m.churnInodeOnWrite {
		f.inode = m.nextIno
		m.nextIno++
	}
	m.tick++
	f.modTick = m.tick
	f.data = append([]byte(nil), data...)
	return nil
}

// Rename moves a file, preserving its inode so document identity survives the
// rename (§1.4.6). The target is overwritten if it exists (mirrors os.Rename).
func (m *Mem) Rename(oldPath, newPath string) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	f := m.files[oldPath]
	if f == nil {
		return &fs.PathError{Op: "rename", Path: oldPath, Err: fs.ErrNotExist}
	}
	delete(m.files, oldPath)
	m.files[newPath] = f
	return nil
}

func (m *Mem) Stat(name string) (fs.FileInfo, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	f := m.files[name]
	if f == nil {
		return nil, &fs.PathError{Op: "stat", Path: name, Err: fs.ErrNotExist}
	}
	return memFileInfo{
		name: filepath.Base(name),
		size: int64(len(f.data)),
		mod:  f.modTick,
		id:   Identity{Inode: f.inode, Device: f.device},
	}, nil
}

// MkdirAll is a no-op: Mem has no directory tree, only flat path→content keys.
func (m *Mem) MkdirAll(string, fs.FileMode) error { return nil }

// Trash removes all entries at path (file) or under path (directory) from the
// in-memory map, mirroring Disk.Trash for tests without touching real disk.
func (m *Mem) Trash(path string) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	clean := filepath.Clean(path)
	prefix := clean + "/"
	found := false
	for k := range m.files {
		if k == clean || strings.HasPrefix(k, prefix) {
			delete(m.files, k)
			found = true
		}
	}
	if !found {
		return &fs.PathError{Op: "trash", Path: path, Err: fs.ErrNotExist}
	}
	return nil
}

// ReadDir lists the direct children of dir, synthesizing intermediate
// directories from the flat path map. Like os.ReadDir, entries are sorted by
// name; a directory with no entries returns an empty slice (not an error).
func (m *Mem) ReadDir(dir string) ([]fs.DirEntry, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	prefix := filepath.Clean(dir)
	if prefix != "/" {
		prefix += "/"
	}
	seen := make(map[string]bool)
	var entries []fs.DirEntry
	for p := range m.files {
		if !strings.HasPrefix(p, prefix) {
			continue
		}
		rest := p[len(prefix):]
		if rest == "" {
			continue
		}
		name, isDir := rest, false
		if i := strings.IndexByte(rest, '/'); i >= 0 {
			name, isDir = rest[:i], true // an intermediate directory
		}
		if seen[name] {
			continue
		}
		seen[name] = true
		entries = append(entries, memDirEntry{name: name, dir: isDir})
	}
	sort.Slice(entries, func(i, j int) bool { return entries[i].Name() < entries[j].Name() })
	return entries, nil
}

// Exchange swaps the file objects at keys a and b so identity (inode) travels
// with content, mimicking a physical renamex_np(RENAME_SWAP) — matches Disk's
// capture-before-discard property: neither key is ever deleted, only
// repointed. Both keys must already exist. Advances the modification clock
// for both entries so the §1.4.7 divergence guard sees the swap as a change.
func (m *Mem) Exchange(a, b string) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	fa, ok := m.files[a]
	if !ok {
		return &fs.PathError{Op: "exchange", Path: a, Err: fs.ErrNotExist}
	}
	fb, ok := m.files[b]
	if !ok {
		return &fs.PathError{Op: "exchange", Path: b, Err: fs.ErrNotExist}
	}
	m.tick++
	fa.modTick = m.tick
	fb.modTick = m.tick
	m.files[a], m.files[b] = fb, fa
	return nil
}

// RenameExcl renames oldPath to newPath, failing with fs.ErrExist if newPath
// is already occupied (mirrors Disk's no-clobber atomic rename).
func (m *Mem) RenameExcl(oldPath, newPath string) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	f, ok := m.files[oldPath]
	if !ok {
		return &fs.PathError{Op: "renameexcl", Path: oldPath, Err: fs.ErrNotExist}
	}
	if _, exists := m.files[newPath]; exists {
		return &fs.PathError{Op: "renameexcl", Path: newPath, Err: fs.ErrExist}
	}
	delete(m.files, oldPath)
	m.files[newPath] = f
	return nil
}

// Remove deletes the single entry at path.
func (m *Mem) Remove(path string) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	if _, ok := m.files[path]; !ok {
		return &fs.PathError{Op: "remove", Path: path, Err: fs.ErrNotExist}
	}
	delete(m.files, path)
	return nil
}

// Resolve is the identity function: Mem has no symlinks.
func (m *Mem) Resolve(path string) (string, error) { return path, nil }

// memDirEntry implements fs.DirEntry for Mem listings.
type memDirEntry struct {
	name string
	dir  bool
}

func (e memDirEntry) Name() string { return e.name }
func (e memDirEntry) IsDir() bool  { return e.dir }
func (e memDirEntry) Type() fs.FileMode {
	if e.dir {
		return fs.ModeDir
	}
	return 0
}
func (e memDirEntry) Info() (fs.FileInfo, error) {
	mode := fs.FileMode(0o644)
	if e.dir {
		mode = fs.ModeDir | 0o755
	}
	return memDirInfo{name: e.name, mode: mode}, nil
}

// memDirInfo is a minimal fs.FileInfo for a ReadDir entry (workspace listing
// uses only Name/IsDir; Info is provided for interface completeness).
type memDirInfo struct {
	name string
	mode fs.FileMode
}

func (i memDirInfo) Name() string       { return i.name }
func (i memDirInfo) Size() int64        { return 0 }
func (i memDirInfo) Mode() fs.FileMode  { return i.mode }
func (i memDirInfo) ModTime() time.Time { return time.Unix(0, 0) }
func (i memDirInfo) IsDir() bool        { return i.mode.IsDir() }
func (i memDirInfo) Sys() any           { return nil }

// memFileInfo implements fs.FileInfo for Mem entries. Sys() returns *Identity so
// vfs.FileID can read the synthetic inode/device.
type memFileInfo struct {
	name string
	size int64
	mod  int64
	id   Identity
}

func (fi memFileInfo) Name() string       { return fi.name }
func (fi memFileInfo) Size() int64        { return fi.size }
func (fi memFileInfo) Mode() fs.FileMode  { return 0o644 }
func (fi memFileInfo) ModTime() time.Time { return time.Unix(0, fi.mod) }
func (fi memFileInfo) IsDir() bool        { return false }
func (fi memFileInfo) Sys() any           { id := fi.id; return &id }
