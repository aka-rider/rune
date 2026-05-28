package dictation

import (
	tea "charm.land/bubbletea/v2"

	"rune/pkg/whisper"
)

// ReadyMsg is returned by StartCmd on successful mic init.
// Ch is the channel workspace must pass to ListenCmd.
type ReadyMsg struct{ Ch <-chan tea.Msg }

// PartialTranscriptionMsg carries the full accumulated dictation text so far.
// It is emitted after each 2-second chunk is transcribed.
type PartialTranscriptionMsg struct{ Accumulated string }

// FinalTranscriptionMsg is emitted once after the user stops dictation and the
// goroutine drains. Text is the complete accumulated transcript.
type FinalTranscriptionMsg struct{ Text string }

// ErrorMsg signals a transcription failure. Fatal=true means mic init failed
// and dictation cannot continue; Fatal=false means a single chunk failed (server
// timeout, network blip) and the session continues.
type ErrorMsg struct {
	Err   error
	Fatal bool
}

// Config holds the runtime dependencies for a dictation session.
type Config struct {
	Whisper  whisper.Client
	Language string // BCP-47 code captured at dictation start; "" means auto-detect
}

// ListenCmd reads one message from the dictation channel.
// Workspace must reschedule ListenCmd after every PartialTranscriptionMsg and
// non-fatal ErrorMsg. Returns nil when the channel is closed (after
// FinalTranscriptionMsg is delivered).
func ListenCmd(ch <-chan tea.Msg) tea.Cmd {
	return func() tea.Msg {
		msg, ok := <-ch
		if !ok {
			return nil // goroutine exited; FinalTranscriptionMsg was already sent
		}
		return msg
	}
}
