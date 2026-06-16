//go:build fuzzing

package footer

import "time"

var (
	errorDismissDelay time.Duration
	confirmDelay      time.Duration
)
