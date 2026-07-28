package footer

// DisableTimersForTesting zeroes footer's real-time auto-dismiss
// (errorDismissDelay, footer_timers.go) and confirm-key (confirmDelay)
// delays for the remainder of the process. Exported from a regular
// (non-_test.go) file — not just footer's own tests — so an IMPORTING
// package's test suite can silence these timers too (mirrors
// docstate.NewTestStore's cross-package test-seam convention); production
// code never calls this. Takes no parameters: both vars exist for two
// distinct UI mechanisms (banner auto-dismiss vs. guard confirm-key expiry)
// but nothing about testing ever wants one real and one zeroed — both are
// wired into a tea.Cmd's time.Sleep purely to stop a recursive Cmd drain
// from paying a real wall-clock delay, so any caller needing one needs both.
//
// A caller that recursively drains every Cmd a footer.Update produces —
// e.g. rune/pkg/ui/pages/workspace's drainCmd — pays the real wall-clock
// delay whenever a guard/error/status path fires one of these timers,
// exactly the same test-seam gap workspace's own flushDelay var has
// (workspace_timers.go) and fixes via TestMain.
func DisableTimersForTesting() {
	errorDismissDelay = 0
	confirmDelay = 0
}
