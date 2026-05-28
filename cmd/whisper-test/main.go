// whisper-test verifies the whisper.cpp HTTP transcription endpoint.
// Usage: go run ./cmd/whisper-test
// Requires sample.wav in the repo root and a running whisper server on :2022.
package main

import (
	"context"
	"fmt"
	"os"

	"rune/pkg/whisper"
)

func main() {
	data, err := os.ReadFile("sample.wav")
	if err != nil {
		fmt.Fprintf(os.Stderr, "read sample.wav: %v\n", err)
		os.Exit(1)
	}

	c := whisper.Client{
		BaseURL:       "http://127.0.0.1:2022",
		InferencePath: "/v1/audio/transcriptions",
	}

	text, err := c.Transcribe(context.Background(), data, "")
	if err != nil {
		fmt.Fprintf(os.Stderr, "transcribe: %v\n", err)
		os.Exit(1)
	}

	fmt.Println(text)
}
