//go:build unix

package vfs

import (
	"io/fs"
	"syscall"
)

// sysFileID extracts (inode, device) from an os.Stat FileInfo via its
// *syscall.Stat_t. Returns ok=false if the underlying Sys() is not a Stat_t.
func sysFileID(fi fs.FileInfo) (inode, device uint64, ok bool) {
	st, isStat := fi.Sys().(*syscall.Stat_t)
	if !isStat {
		return 0, 0, false
	}
	return uint64(st.Ino), uint64(st.Dev), true
}

// sysNLink extracts the hard-link count from an os.Stat FileInfo via its
// *syscall.Stat_t. Returns ok=false if the underlying Sys() is not a
// Stat_t.
func sysNLink(fi fs.FileInfo) (nlink uint64, ok bool) {
	st, isStat := fi.Sys().(*syscall.Stat_t)
	if !isStat {
		return 0, false
	}
	return uint64(st.Nlink), true
}
