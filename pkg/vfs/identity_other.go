//go:build !unix

package vfs

import "io/fs"

// sysFileID returns ok=false on non-unix platforms; callers degrade to
// path-keying (mirrors the former docstate fileid_other.go).
func sysFileID(_ fs.FileInfo) (inode, device uint64, ok bool) {
	return 0, 0, false
}

// sysNLink returns ok=false on non-unix platforms.
func sysNLink(_ fs.FileInfo) (nlink uint64, ok bool) {
	return 0, false
}
