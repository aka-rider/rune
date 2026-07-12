package driver

import (
	"os"
	"testing"

	"rune/internal/fuzz/harness"
)

// TestMain arms every Category 3 test/fuzz hook (harness.Hermetic) before
// any test in this package runs. driver.bootstrap drives a fresh
// workspace.Model through Init() synchronously (drainCmd), which — now that
// openStore/runOpener/ai.NewClient/etc. are untagged, default-on production
// behavior rather than build-tag-gated no-ops — would otherwise open a real
// on-disk SQLite store (and race the deliberately-injected in-memory
// StoreReadyMsg that follows) before a single fuzz driver test runs.
//
// Safe here as an internal (package driver) file: harness imports
// rune/pkg/ui/pages/workspace, and workspace does not import
// rune/internal/fuzz/driver, so no import cycle — unlike the workspace
// package's own tests, which must keep this in an external file (see
// pkg/ui/pages/workspace/main_test.go).
func TestMain(m *testing.M) {
	harness.Hermetic()
	os.Exit(m.Run())
}
