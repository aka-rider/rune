//go:build fuzzing

package dictation

import (
	"context"

	tea "charm.land/bubbletea/v2"
)

// StartCmd is the fuzzing-build stub: no real microphone/whisper pipeline
// exists under -tags fuzzing (§1.4.9's watcher/fuzz-tooling exception
// family — same shape as workspace_watch_fuzz.go). It returns a
// deterministic, non-blocking ReadyMsg{Ch} over an already-CLOSED channel,
// matching the real StartCmd's contract (ReadyMsg on success) without
// spawning any goroutine or touching a real device: ListenCmd
// (messages.go) reads from a closed channel and returns nil (ok=false),
// so the caller's re-schedule loop terminates immediately instead of
// blocking forever waiting for a message that would never arrive. Actual
// dictation content under fuzzing arrives via KindDictation's direct
// PartialTranscriptionMsg/FinalTranscriptionMsg/ErrorMsg injection
// (internal/fuzz/driver/driver_human.go) — this stub only makes the ⌃V
// enable path (workspace_update.go's StartCmd call) safe to reach at all.
func StartCmd(_ context.Context, _ Config) tea.Cmd {
	ch := make(chan tea.Msg)
	close(ch)
	return func() tea.Msg {
		return ReadyMsg{Ch: ch}
	}
}
