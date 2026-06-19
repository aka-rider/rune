//go:build !unix

package docstate

// fileID returns (0, 0, false) on non-unix platforms. OpenPath degrades to
// path-keying when ok is false.
func fileID(_ string) (inode, device uint64, ok bool) {
	return 0, 0, false
}
