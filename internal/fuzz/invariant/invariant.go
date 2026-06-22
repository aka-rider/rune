// Package invariant provides the Violation type (the output of every invariant
// check) and the Monitor interface for stateful L2 monitors. All invariant
// logic lives in per-domain checker packages under internal/fuzz/{ui,editor}/…;
// the session aggregator at internal/fuzz/session composes them.
package invariant

// Violation records a failed invariant check.
type Violation struct {
	InvariantID string
	Message     string
}

// trunc truncates s to at most n bytes, appending "…" if cut.
// Exported so checker packages can reuse it without redefining it.
func Trunc(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n] + "…"
}
