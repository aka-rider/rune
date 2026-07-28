package workspace_test

import (
	"os"
	"testing"

	"rune/internal/fuzz/harness"
)

// TestMain arms every Category 3 test/fuzz hook (real timers, real disk
// store open, real OS opener process, real network client, real
// microphone/whisper pipeline) via the single harness.Hermetic()
// chokepoint, for the whole test binary run — every *_test.go in this
// package, internal (package workspace) or external (package
// workspace_test), shares this one test binary and this one TestMain.
//
// This file MUST be external (package workspace_test): harness imports
// rune/pkg/ui/pages/workspace, so a package-internal test file importing
// harness would be an import cycle ("import cycle not allowed in test",
// critic-verified empirically) — see harness.go's doc comment.
func TestMain(m *testing.M) {
	harness.Hermetic()
	os.Exit(m.Run())
}
