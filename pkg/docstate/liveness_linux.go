//go:build linux

package docstate

import (
	"fmt"
	"os"
	"strconv"
	"strings"
)

// processStartedAt reads field 22 (starttime, in clock ticks since boot) of
// /proc/<pid>/stat — the linux equivalent of darwin's sysctl(KERN_PROC)
// start time (liveness_darwin.go). The comm field (field 2) is
// parenthesized and may itself contain spaces or parens, so this locates
// the LAST ')' in the line and splits only the remainder on whitespace,
// rather than naively splitting the whole line — a comm like "a) (b)" would
// otherwise throw off a fixed field index. Returns ok=false (never a
// positive claim) on any read/parse failure, including "no such process"
// (os.ReadFile's ENOENT) — EXISTENCE is decided separately and portably via
// processExists (liveness_unix.go, kill(pid,0)); this function is only ever
// consulted once existence is already established, purely to detect pid
// reuse.
func processStartedAt(pid int) (string, bool) {
	data, err := os.ReadFile(fmt.Sprintf("/proc/%d/stat", pid))
	if err != nil {
		return "", false
	}
	s := string(data)
	i := strings.LastIndexByte(s, ')')
	if i < 0 || i+2 >= len(s) {
		return "", false
	}
	fields := strings.Fields(s[i+2:])
	// fields[0] is state (stat field 3, 1-indexed); starttime is field 22,
	// i.e. fields[22-3] = fields[19].
	const starttimeIdx = 22 - 3
	if len(fields) <= starttimeIdx {
		return "", false
	}
	starttime := fields[starttimeIdx]
	if _, err := strconv.ParseInt(starttime, 10, 64); err != nil {
		return "", false
	}
	return starttime, true
}
