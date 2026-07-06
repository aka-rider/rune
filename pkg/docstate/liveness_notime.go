//go:build !linux && !darwin

package docstate

// processStartedAt is unavailable on this platform — isProcessAlive degrades
// to existence-only comparison (processExists) and never positively
// confirms identity across a pid-reuse boundary here; Store construction
// records an empty proc_started_at, which isProcessAlive treats as "fail
// toward alive".
func processStartedAt(_ int) (string, bool) {
	return "", false
}
