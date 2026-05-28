//go:build darwin

package dictation

import (
	"context"
	"strings"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/microphone"
	whisperPkg "rune/pkg/whisper"
)

// StartCmd starts the microphone capture goroutine.
// On success it returns ReadyMsg{Ch}; on mic init failure it returns ErrorMsg{Fatal:true}.
// Workspace must cancel ctx to stop the session (which triggers FinalTranscriptionMsg).
func StartCmd(ctx context.Context, cfg Config) tea.Cmd {
	return func() tea.Msg {
		micCh, err := microphone.Start(ctx)
		if err != nil {
			return ErrorMsg{Err: err, Fatal: true}
		}

		out := make(chan tea.Msg, 8)

		go func() {
			defer close(out)

			var accumulator string

			for {
				select {
				case <-ctx.Done():
					out <- FinalTranscriptionMsg{Text: strings.TrimSpace(accumulator)}
					return

				case chunk, ok := <-micCh:
					if !ok {
						out <- FinalTranscriptionMsg{Text: strings.TrimSpace(accumulator)}
						return
					}

					wav := whisperPkg.EncodePCM(chunk, 16000, 1, 16)

					// Capture Language() return value before the closure executes (§5.4).
					lang := cfg.Language()

					text, err := cfg.Whisper.Transcribe(ctx, wav, lang)
					if err != nil {
						out <- ErrorMsg{Err: err, Fatal: false}
						continue
					}

					accumulator += text + " "
					out <- PartialTranscriptionMsg{Accumulated: strings.TrimSpace(accumulator)}
				}
			}
		}()

		return ReadyMsg{Ch: out}
	}
}
