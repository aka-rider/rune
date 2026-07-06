// mic-wav records from the default microphone and writes a WAV file.
// Usage: go run ./cmd/mic-wav [-d seconds] [-o output.wav]
package main

import (
	"context"
	"flag"
	"fmt"
	"os"
	"time"

	"rune/pkg/dictation"
	"rune/pkg/microphone"
)

func main() {
	duration := flag.Int("d", 5, "recording duration in seconds")
	output := flag.String("o", "recording.wav", "output WAV file path")
	flag.Parse()

	fmt.Printf("Recording %d seconds → %s\n", *duration, *output)
	fmt.Println("Speak now...")

	ctx, cancel := context.WithTimeout(context.Background(), time.Duration(*duration)*time.Second+3*time.Second)
	defer cancel()

	ch, err := microphone.Start(ctx)
	if err != nil {
		fmt.Fprintf(os.Stderr, "mic init: %v\n", err)
		os.Exit(1)
	}

	var pcm []byte
	deadline := time.After(time.Duration(*duration) * time.Second)

loop:
	for {
		select {
		case <-deadline:
			cancel()
			break loop
		case chunk, ok := <-ch:
			if !ok {
				break loop
			}
			pcm = append(pcm, chunk...)
		}
	}

	if len(pcm) == 0 {
		fmt.Fprintln(os.Stderr, "no audio captured")
		os.Exit(1)
	}

	wav := dictation.EncodePCM(pcm, 16000, 1, 16)
	if err := os.WriteFile(*output, wav, 0644); err != nil {
		fmt.Fprintf(os.Stderr, "write: %v\n", err)
		os.Exit(1)
	}

	seconds := float64(len(pcm)) / (16000 * 2)
	fmt.Printf("Done. Wrote %.1fs of audio (%d bytes) to %s\n", seconds, len(wav), *output)
	fmt.Printf("Play back with: afplay %s\n", *output)
}
