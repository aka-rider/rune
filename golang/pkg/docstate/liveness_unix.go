//go:build unix

package docstate

import "syscall"

// processExists reports whether pid currently identifies a running process,
// via the POSIX kill(pid, 0) idiom (sends no signal, only checks
// addressability) — the one existence check that behaves identically and
// unambiguously on every unix (mirrors vfs's own //go:build unix precedent,
// identity_unix.go), unlike each platform's own start-time read
// (processStartedAt), which on darwin cannot cleanly distinguish "no such
// process" from an unrelated read failure. found=false means POSITIVELY
// confirmed gone (ESRCH); ok=false means the check itself was inconclusive
// (any other errno) — the caller must fail toward "alive" in that case,
// never toward "dead".
func processExists(pid int) (found, ok bool) {
	err := syscall.Kill(pid, 0)
	switch err {
	case nil:
		return true, true // exists, and we can signal it
	case syscall.ESRCH:
		return false, true // positively confirmed: no such process
	case syscall.EPERM:
		return true, true // exists, owned by another user — still a real process
	default:
		return false, false // inconclusive
	}
}
