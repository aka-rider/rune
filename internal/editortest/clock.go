package editortest

import "time"

// Clock is a deterministic clock for testing.
type Clock struct {
	now time.Time
}

// NewClock returns a Clock starting at a fixed time.
func NewClock() Clock {
	return Clock{now: time.Date(2024, 1, 1, 0, 0, 0, 0, time.UTC)}
}

// Now returns the current time of the clock.
func (c Clock) Now() time.Time {
	return c.now
}

// Advance returns a new Clock advanced by the given duration.
func (c Clock) Advance(d time.Duration) Clock {
	return Clock{now: c.now.Add(d)}
}

// AutoClock returns a deterministic, strictly monotonic clock function:
// each call returns the previous time advanced by step, starting from the
// same fixed epoch as NewClock. It replaces time.Now wherever a store's
// clock feeds behavior a test must replay identically — most critically
// docstate.AppendEdit's 300ms coalescing window, where a real clock makes
// the SAME event sequence coalesce or not depending on machine load
// (observed: a fuzz corpus entry flipping pass/fail with load). A
// millisecond step keeps consecutive replayed keystrokes inside the window
// — the regime a fast human typist produces — while never yielding two
// equal timestamps. Not safe for concurrent use: fuzz/scenario replay is
// single-goroutine by construction (the driver drains every Cmd inline).
func AutoClock(step time.Duration) func() time.Time {
	now := time.Date(2024, 1, 1, 0, 0, 0, 0, time.UTC)
	return func() time.Time {
		now = now.Add(step)
		return now
	}
}
