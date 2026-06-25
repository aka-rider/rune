//go:build !fuzzing

package workspace

import "os/exec"

// runOpener launches the OS default handler for an external link. The command and
// args come from externalOpener (per-GOOS), and the URL is always a separate exec
// argument — never a shell string — so a crafted link cannot inject a command
// (markdownedit.isExternalURL already bounds the scheme to http(s)/mailto).
func runOpener(name string, args ...string) error {
	return exec.Command(name, args...).Start()
}
