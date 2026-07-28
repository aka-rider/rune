package dictation

import (
	"context"

	tea "charm.land/bubbletea/v2"
)

// startStub, when non-nil, replaces the real microphone/whisper pipeline
// (StartCmd, start_darwin.go) for the remainder of the process. Set via
// UseStubForTesting; nil in production.
var startStub func(context.Context, Config) tea.Cmd

// UseStubForTesting installs a deterministic, non-blocking stub for StartCmd
// that returns ReadyMsg{Ch} over an already-CLOSED channel, matching the
// real StartCmd's contract (ReadyMsg on success) without spawning any
// goroutine or touching a real device: ListenCmd (messages.go) reads from a
// closed channel and returns nil (ok=false), so the caller's re-schedule
// loop terminates immediately instead of blocking forever waiting for a
// message that would never arrive. Actual dictation content in a test/fuzz
// run arrives via KindDictation's direct PartialTranscriptionMsg/
// FinalTranscriptionMsg/ErrorMsg injection (internal/fuzz/driver/
// driver_human.go) — this stub only makes the ⌃V enable path
// (workspace_update.go's StartCmd call) safe to reach at all. Exported from
// a regular (non-_test.go) file — mirrors footer.DisableTimersForTesting
// (footer_testing.go) — so an importing package's test suite (e.g.
// internal/fuzz/harness) can install it too; production code never calls
// this.
func UseStubForTesting() {
	startStub = func(context.Context, Config) tea.Cmd {
		ch := make(chan tea.Msg)
		close(ch)
		return func() tea.Msg {
			return ReadyMsg{Ch: ch}
		}
	}
}
