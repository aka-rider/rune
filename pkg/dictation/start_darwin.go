//go:build darwin

package dictation

import (
	"context"
	"strings"
	"time"

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

			var allAudio []byte
			var lastText string
			lang := cfg.Language

			for {
				select {
				case <-ctx.Done():
					if len(allAudio) > 0 {
						// Use a fresh context for the final transcription since ctx is cancelled.
						finalCtx, finalCancel := context.WithTimeout(context.Background(), 30*time.Second)
						wav := whisperPkg.EncodePCM(allAudio, 16000, 1, 16)
						text, err := cfg.Whisper.Transcribe(finalCtx, wav, lang)
						finalCancel()
						if err == nil && strings.TrimSpace(text) != "" {
							lastText = strings.TrimSpace(text)
						}
					}
					out <- FinalTranscriptionMsg{Text: lastText}
					return

				case chunk, ok := <-micCh:
					if !ok {
						out <- FinalTranscriptionMsg{Text: lastText}
						return
					}
					allAudio = append(allAudio, chunk...)

					wav := whisperPkg.EncodePCM(allAudio, 16000, 1, 16)
					text, err := cfg.Whisper.Transcribe(ctx, wav, lang)
					if err != nil {
						out <- ErrorMsg{Err: err, Fatal: false}
						continue
					}

					trimmed := strings.TrimSpace(text)
					if trimmed != lastText {
						lastText = trimmed
						out <- PartialTranscriptionMsg{Accumulated: lastText}
					}
				}
			}
		}()

		return ReadyMsg{Ch: out}
	}
}
