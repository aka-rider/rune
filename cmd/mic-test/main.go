// mic-test verifies the microphone capture and whisper transcription pipeline.
// Usage: go run ./cmd/mic-test
// Records three 2-second chunks, transcribes each, and prints results.
package main

import (
	"context"
	"fmt"
	"os"

	"rune/pkg/microphone"
	"rune/pkg/whisper"
)

func main() {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	ch, err := microphone.Start(ctx)
	if err != nil {
		fmt.Fprintf(os.Stderr, "mic init: %v\n", err)
		os.Exit(1)
	}

	c := whisper.Client{
		BaseURL:       "http://127.0.0.1:8080",
		InferencePath: "/inference",
	}

	count := 0
	for chunk := range ch {
		if count >= 3 {
			cancel()
			break
		}
		wav := whisper.EncodePCM(chunk, 16000, 1, 16)
		text, err := c.Transcribe(context.Background(), wav, "")
		if err != nil {
			fmt.Fprintf(os.Stderr, "chunk %d: transcribe: %v\n", count+1, err)
		} else {
			fmt.Printf("chunk %d: %s\n", count+1, text)
		}
		count++
	}
}
