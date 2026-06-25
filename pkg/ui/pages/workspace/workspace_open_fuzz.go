//go:build fuzzing

package workspace

// runOpener is a no-op under the fuzzing build tag: following an external link must
// never spawn a real browser/opener process during a fuzz run (the driver executes
// returned Cmds synchronously). The LinkExternal branch — dispatch + footer status —
// is still exercised; only the process spawn is suppressed.
func runOpener(_ string, _ ...string) error { return nil }
