package workspace

import "os/exec"

// runOpener is a function-var indirection over realRunOpener (workspace_nav.go's
// LinkExternal call site) so DisableOpenerForTesting can swap it for a no-op —
// following an external link must never spawn a real browser/opener process
// during a test or fuzz run (the driver/settle loop executes returned Cmds
// synchronously). The LinkExternal branch — dispatch + footer status — is
// still exercised; only the process spawn is suppressed.
var runOpener = realRunOpener

// DisableOpenerForTesting replaces runOpener with a no-op for the remainder
// of the process. Exported from a regular (non-_test.go) file — mirrors
// footer.DisableTimersForTesting (footer_testing.go) — so an importing
// package's test suite (e.g. internal/fuzz/harness) can silence it too;
// production code never calls this.
func DisableOpenerForTesting() {
	runOpener = func(string, ...string) error { return nil }
}

// realRunOpener launches the OS default handler for an external link. The command and
// args come from externalOpener (per-GOOS), and the URL is always a separate exec
// argument — never a shell string — so a crafted link cannot inject a command
// (markdownedit.isExternalURL already bounds the scheme to http(s)/mailto).
func realRunOpener(name string, args ...string) error {
	return exec.Command(name, args...).Start()
}
