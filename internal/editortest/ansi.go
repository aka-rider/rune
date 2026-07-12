package editortest

import "github.com/charmbracelet/x/ansi"

// StripANSI removes ANSI escape sequences from s for plain-text view
// assertions. Single chokepoint over charmbracelet/x/ansi.Strip (already a
// module dependency) — replaces per-package hand-rolled CSI skippers.
func StripANSI(s string) string {
	return ansi.Strip(s)
}
