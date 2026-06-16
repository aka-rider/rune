//go:build !fuzzing

package footer

import "time"

var (
	errorDismissDelay = 5 * time.Second
	confirmDelay      = 2 * time.Second
)
