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
