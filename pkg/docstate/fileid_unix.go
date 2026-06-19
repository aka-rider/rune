//go:build unix

package docstate

import "syscall"

// fileID returns the (inode, device) identity of the file at path.
// Returns ok=false if stat fails.
func fileID(path string) (inode, device uint64, ok bool) {
	var st syscall.Stat_t
	if err := syscall.Stat(path, &st); err != nil {
		return 0, 0, false
	}
	return uint64(st.Ino), uint64(st.Dev), true
}
