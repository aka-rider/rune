//go:build !unix

package docstate

// processExists is unavailable on this platform — isProcessAlive degrades to
// its inconclusive (ok=false) path, which fails toward "alive" (never
// auto-adopts without positive confirmation of death).
func processExists(_ int) (found, ok bool) {
	return false, false
}
