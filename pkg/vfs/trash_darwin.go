//go:build darwin

package vfs

import "os/exec"

func (Disk) Trash(path string) error {
	return exec.Command("/usr/bin/trash", path).Run()
}
