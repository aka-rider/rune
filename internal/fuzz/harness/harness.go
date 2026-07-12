// Package harness is the single chokepoint that arms every Category 3
// test/fuzz hook (see the QA-rehaul plan, Phase 1) needed to make the
// workspace safe to drive synchronously and hermetically — no real timers,
// no real disk store open, no real OS opener process, no real network
// client, no real microphone/whisper pipeline.
//
// harness imports pkg/ui/pages/workspace (transitively, via
// DisableTimersForTesting/DisableStoreOpenForTesting/DisableOpenerForTesting
// living in that package), so it must NEVER be imported from a
// package-internal (package workspace) test file — Go rejects that as an
// "import cycle not allowed in test" (critic-verified empirically). Callers
// that need Hermetic() from within the workspace package's own tests must
// live in an external `package workspace_test` file instead (see
// pkg/ui/pages/workspace/main_test.go).
package harness

import (
	"rune/pkg/ai"
	"rune/pkg/dictation"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/components/image"
	"rune/pkg/ui/pages/workspace"
)

// Hermetic arms every hook that makes the workspace (and everything it can
// reach through a real key/message path) safe to drive synchronously,
// without spawning a real timer, disk store, OS opener process, network
// client, or microphone/whisper pipeline. Idempotent; call once per test
// binary (from a TestMain), before any test runs.
func Hermetic() {
	workspace.DisableTimersForTesting()
	footer.DisableTimersForTesting()
	image.DisableFrameDelayForTesting()
	workspace.DisableStoreOpenForTesting()
	workspace.DisableOpenerForTesting()
	ai.DisableForTesting()
	dictation.UseStubForTesting()
}
