package workspace

import "time"

// flushDelay is the production autosave debounce (workspace_journal.go's
// scheduleFlush). DisableTimersForTesting zeroes it for the remainder of the
// process — mirrors footer.DisableTimersForTesting (footer_testing.go); the
// debounce-staleness check (msg.gen == m.flushGen, workspace_update.go) is a
// generation counter, not wall-clock-based, so flushDelay=0 is behaviorally
// identical under synchronous draining, just instant instead of slow.
var flushDelay = 2 * time.Second

// DisableTimersForTesting zeroes flushDelay for the remainder of the
// process. Exported from a regular (non-_test.go) file so an importing
// package's test suite (e.g. internal/fuzz/harness) can silence it too;
// production code never calls this.
func DisableTimersForTesting() {
	flushDelay = 0
}
