package image

// DisableFrameDelayForTesting collapses scheduleFrame's real frame-advance
// delay (image_tick.go) to zero for the remainder of the process. Exported
// from a regular (non-_test.go) file — mirrors
// footer.DisableTimersForTesting (footer_testing.go) — so an importing
// package's test suite (e.g. internal/fuzz/harness) can silence it too;
// production code never calls this.
func DisableFrameDelayForTesting() {
	frameDelayDisabled = true
}

// DisableTTYWritesForTesting turns writeTTY (commands.go) into a successful
// no-op for the remainder of the process, so tests can execute real
// Transmit*/Delete*Cmd values — receiving their correctly Gen-stamped result
// messages — without writing escape bytes to the developer's terminal (or
// failing when the runner has no controlling TTY). Same pattern as
// DisableFrameDelayForTesting above; production code never calls this.
func DisableTTYWritesForTesting() {
	ttyWritesDisabled = true
}
