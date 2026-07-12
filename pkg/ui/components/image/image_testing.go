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
