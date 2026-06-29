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
type Mem struct {
	mu      sync.Mutex
	files   map[string]*memFile
	nextIno uint64
	tick    int64 // monotonic source for ModTime; advances on every write
}

type memFile struct {
	data    []byte
	inode   uint64
	device  uint64
	modTick int64
}

// NewMem returns an empty in-memory filesystem.
func NewMem() *Mem {
	return &Mem{files: make(map[string]*memFile), nextIno: 1}
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
// time a path is written and advancing the modification clock every time.
func (m *Mem) WriteFile(name string, data []byte, _ fs.FileMode) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	f := m.files[name]
	if f == nil {
		f = &memFile{inode: m.nextIno, device: 1}
		m.nextIno++
		m.files[name] = f
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
func (i memDirInfo) Size() int64         { return 0 }
func (i memDirInfo) Mode() fs.FileMode   { return i.mode }
func (i memDirInfo) ModTime() time.Time  { return time.Unix(0, 0) }
func (i memDirInfo) IsDir() bool         { return i.mode.IsDir() }
func (i memDirInfo) Sys() any            { return nil }

// memFileInfo implements fs.FileInfo for Mem entries. Sys() returns *Identity so
// vfs.FileID can read the synthetic inode/device.
type memFileInfo struct {
	name string
	size int64
	mod  int64
	id   Identity
}

func (fi memFileInfo) Name() string       { return fi.name }
func (fi memFileInfo) Size() int64         { return fi.size }
func (fi memFileInfo) Mode() fs.FileMode   { return 0o644 }
func (fi memFileInfo) ModTime() time.Time  { return time.Unix(0, fi.mod) }
func (fi memFileInfo) IsDir() bool         { return false }
func (fi memFileInfo) Sys() any            { id := fi.id; return &id }
