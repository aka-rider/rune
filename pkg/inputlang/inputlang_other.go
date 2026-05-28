//go:build !darwin

package inputlang

// Current returns the BCP-47 language code of the active keyboard input source.
// On non-darwin platforms, returns "" (server will auto-detect language).
func Current() string { return "" }
