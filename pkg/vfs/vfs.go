// Package vfs is the single chokepoint for real-disk file I/O of the user's
// .md documents: read, write, rename, stat. Everything funnels through the FS
// interface so the production build uses Disk (os.* + atomic write) while tests
// and the session fuzzer use Mem (fully in-memory), exercising the complete
// load→edit→save→rename→reopen machinery with no real disk touched.
//
// The interface is intentionally go-native: method shapes mirror os.* and the
// types are stdlib (fs.FileInfo, fs.FileMode, fs.ErrNotExist). Atomicity is an
// implementation detail of Disk, not part of the contract.
package vfs

import (
	"io/fs"
	"os"

	"rune/pkg/atomicfile"
)

// FS abstracts the filesystem operations the workspace performs on .md files.
// Paths are absolute OS paths (not io/fs unrooted names), so FS does not embed
// fs.FS — that would impose fs.ValidPath and reject absolute paths. Missing
// files surface as an error wrapping fs.ErrNotExist (use errors.Is).
type FS interface {
	ReadFile(name string) ([]byte, error)
	WriteFile(name string, data []byte, perm fs.FileMode) error
	Rename(oldPath, newPath string) error
	Stat(name string) (fs.FileInfo, error)
	MkdirAll(path string, perm fs.FileMode) error
	// ReadDir lists the directory at name, sorted by filename (like os.ReadDir).
	ReadDir(name string) ([]fs.DirEntry, error)
	// Trash moves path to the OS trash. Works for files and directories.
	// Disk delegates to /usr/bin/trash; Mem removes from the in-memory map.
	Trash(path string) error
}

// Identity is the stable (inode, device) identity of a file. History is keyed to
// it rather than the path so a rename does not orphan history (CLAUDE.md §1.4.6).
// Mem exposes it via fs.FileInfo.Sys(); Disk exposes the OS *syscall.Stat_t,
// which sysFileID converts.
type Identity struct {
	Inode  uint64
	Device uint64
}

// FileID extracts (inode, device) from fi. ok is false when the platform or the
// FileInfo does not carry stable identity, in which case callers degrade to
// path-keying. Replaces docstate's former fileID(path) helper.
func FileID(fi fs.FileInfo) (inode, device uint64, ok bool) {
	if fi == nil {
		return 0, 0, false
	}
	if id, isMem := fi.Sys().(*Identity); isMem {
		return id.Inode, id.Device, true
	}
	return sysFileID(fi)
}

// Disk is the production FS: a thin wrapper over os.*. Writes are atomic
// (temp→Sync→Rename→dir-fsync, §1.4.1) via the unchanged pkg/atomicfile.
type Disk struct{}

var _ FS = Disk{}

func (Disk) ReadFile(name string) ([]byte, error) { return os.ReadFile(name) }

// WriteFile writes data atomically. perm is not honored — the atomic helper
// owns the temp file's mode; no caller depends on a specific destination perm.
func (Disk) WriteFile(name string, data []byte, _ fs.FileMode) error {
	return atomicfile.Write(name, data)
}

func (Disk) Rename(oldPath, newPath string) error { return os.Rename(oldPath, newPath) }

func (Disk) Stat(name string) (fs.FileInfo, error) { return os.Stat(name) }

func (Disk) MkdirAll(path string, perm fs.FileMode) error { return os.MkdirAll(path, perm) }

func (Disk) ReadDir(name string) ([]fs.DirEntry, error) { return os.ReadDir(name) }
