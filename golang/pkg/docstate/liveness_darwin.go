//go:build darwin

package docstate

import (
	"fmt"

	"golang.org/x/sys/unix"
)

// processStartedAt reads pid's start time via sysctl(KERN_PROC,
// KERN_PROC_PID) — the darwin equivalent of linux's /proc/<pid>/stat
// starttime field (liveness_linux.go). Returns ok=false (never a positive
// claim) on any error, including "no such process" — darwin's sysctl
// wrapper does not cleanly distinguish that from other I/O failures, so
// EXISTENCE is decided separately and portably via processExists
// (liveness_unix.go, kill(pid,0)); this function is only ever consulted
// once existence is already established, purely to detect pid reuse.
func processStartedAt(pid int) (string, bool) {
	info, err := unix.SysctlKinfoProc("kern.proc.pid", pid)
	if err != nil {
		return "", false
	}
	t := info.Proc.P_starttime
	return fmt.Sprintf("%d.%06d", t.Sec, t.Usec), true
}
