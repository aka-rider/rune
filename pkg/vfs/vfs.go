// Package vfs is the single chokepoint for real-disk file I/O of the user's
// .md documents: read, write, rename, stat. Everything funnels through the FS
// interface so the production build uses Disk (os.* + durable write, atomic
// publish via Exchange/RenameExcl) while tests and the session fuzzer use Mem
// (fully in-memory), exercising the complete load→edit→save→rename→reopen
// machinery with no real disk touched.
//
// The interface is intentionally go-native: method shapes mirror os.* and the
// types are stdlib (fs.FileInfo, fs.FileMode, fs.ErrNotExist). Atomicity is an
// implementation detail of Disk, not part of the contract.
package vfs

import (
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
)

// ErrUnsupported is returned where the underlying filesystem lacks the atomic
// primitive a call needs — e.g. a mount (some SMB/NFS/FAT volumes) whose
// renamex_np rejects RENAME_SWAP/RENAME_EXCL with EOPNOTSUPP, which
// syscall.Errno.Is maps to errors.ErrUnsupported. rune is macOS-only, so this
// is a per-filesystem capability gap, never an OS-portability shim. Aliases the
// stdlib sentinel so callers use the ordinary errors.Is(err, vfs.ErrUnsupported)
// check without a bespoke type (root CLAUDE.md "Go-native interfaces").
var ErrUnsupported = errors.ErrUnsupported

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
	// Exchange atomically swaps the contents of paths a and b (both must
	// exist; same volume). Disk: darwin renamex_np(RENAME_SWAP), fsyncing the
	// parent directory afterward — the durability guarantee for the publish,
	// §1.4.1. Returns an error wrapping ErrUnsupported where the filesystem
	// lacks RENAME_SWAP. Mem: swaps the two entries' file objects between
	// keys (inodes travel with content, mimicking a physical swap) and
	// advances the modification clock.
	Exchange(a, b string) error
	// RenameExcl atomically renames oldPath to newPath, failing with an error
	// wrapping fs.ErrExist if newPath already exists — no clobber. Disk:
	// darwin renamex_np(RENAME_EXCL), wrapping ErrUnsupported where the
	// filesystem lacks RENAME_EXCL. Mem: fails if newPath is already occupied.
	RenameExcl(oldPath, newPath string) error
	// Remove deletes a single file. Disk: os.Remove. Needed because
	// Materialize must clean its swapped-out temp through the injected FS —
	// Trash is semantically wrong for internal temps (it shells to
	// /usr/bin/trash on Disk).
	Remove(path string) error
	// Resolve canonicalizes path. Disk: filepath.EvalSymlinks (so saves write
	// through a symlink to its target, never over the link itself), falling
	// back to resolving only the existing parent when the leaf itself does
	// not exist yet (first save of a new file). Mem: identity. Keeps symlink
	// resolution inside the vfs boundary (§1.4.9) and Mem-based tests working.
	Resolve(path string) (string, error)
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

// FileNLink extracts the hard-link count from fi — the Materialize protocol
// (Part III step 6) surfaces a footer warning when it's greater than 1, since
// a save through a hardlinked path silently forks the document from its other
// names on disk. ok is false when the platform doesn't expose it. Mem has no
// hardlink concept, so every Mem file reports (1, true).
func FileNLink(fi fs.FileInfo) (nlink uint64, ok bool) {
	if fi == nil {
		return 0, false
	}
	if _, isMem := fi.Sys().(*Identity); isMem {
		return 1, true
	}
	return sysNLink(fi)
}

// Disk is the production FS: a thin wrapper over os.*. WriteFile is durable
// (fsync'd) but not itself atomic — atomicity of a publish to a destination
// path is provided by Exchange (overwrite) / RenameExcl (create), §1.4.1.
type Disk struct{}

var _ FS = Disk{}

func (Disk) ReadFile(name string) ([]byte, error) { return os.ReadFile(name) }

// WriteFile creates (or truncates) name, writes data, and fsyncs it before
// closing — durable, but not atomic: callers that need an atomic publish to a
// final destination write to a sibling temp via WriteFile and then Exchange
// or RenameExcl that temp onto the destination (§1.4.1). perm is honored as
// the mode passed to O_CREATE.
func (Disk) WriteFile(name string, data []byte, perm fs.FileMode) error {
	f, err := os.OpenFile(name, os.O_CREATE|os.O_TRUNC|os.O_WRONLY, perm)
	if err != nil {
		return fmt.Errorf("write %q: open: %w", name, err)
	}
	if _, err := f.Write(data); err != nil {
		f.Close() //nolint:errcheck // best-effort close after a write failure; the write error is what matters
		return fmt.Errorf("write %q: write: %w", name, err)
	}
	if err := f.Sync(); err != nil {
		f.Close() //nolint:errcheck // best-effort close after a sync failure; the sync error is what matters
		return fmt.Errorf("write %q: sync: %w", name, err)
	}
	if err := f.Close(); err != nil {
		return fmt.Errorf("write %q: close: %w", name, err)
	}
	return nil
}

func (Disk) Rename(oldPath, newPath string) error { return os.Rename(oldPath, newPath) }

func (Disk) Stat(name string) (fs.FileInfo, error) { return os.Stat(name) }

func (Disk) MkdirAll(path string, perm fs.FileMode) error { return os.MkdirAll(path, perm) }

func (Disk) ReadDir(name string) ([]fs.DirEntry, error) { return os.ReadDir(name) }

// Remove deletes a single file (not Trash — internal temps must never shell
// out to /usr/bin/trash).
func (Disk) Remove(path string) error {
	if err := os.Remove(path); err != nil {
		return fmt.Errorf("remove %q: %w", path, err)
	}
	return nil
}

// Resolve canonicalizes path via filepath.EvalSymlinks. When the leaf itself
// does not exist yet (first save of a brand-new file — EvalSymlinks requires
// every path component to exist), only the parent directory is resolved and
// the unresolved leaf name is re-joined, so a symlinked parent directory still
// canonicalizes correctly.
func (Disk) Resolve(path string) (string, error) {
	resolved, err := filepath.EvalSymlinks(path)
	if err == nil {
		return resolved, nil
	}
	if !errors.Is(err, fs.ErrNotExist) {
		return "", fmt.Errorf("resolve %q: %w", path, err)
	}
	dir, base := filepath.Split(path)
	resolvedDir, dirErr := filepath.EvalSymlinks(filepath.Clean(dir))
	if dirErr != nil {
		return "", fmt.Errorf("resolve %q: resolve parent: %w", path, dirErr)
	}
	return filepath.Join(resolvedDir, base), nil
}

// fsyncDir best-effort fsyncs the parent directory of path so a preceding
// rename/exchange within it survives a crash (§1.4.1).
func fsyncDir(path string) error {
	d, err := os.Open(filepath.Dir(path))
	if err != nil {
		return err
	}
	defer d.Close()
	return d.Sync()
}
