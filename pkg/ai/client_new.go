package ai

import (
	"errors"
	"fmt"
	"os"
)

// disabledForTesting makes NewClient always refuse, regardless of
// environment, once set via DisableForTesting. A hermetic test/fuzz run
// must never drive a real HTTP request from inside a fuzz worker or test
// process (an unbounded wall-clock stall the Go fuzz coordinator would kill
// as "hung or terminated unexpectedly"); the chat component's initErr path
// renders the refusal exactly as it would a missing API key.
var disabledForTesting bool

// DisableForTesting makes NewClient always return an error for the
// remainder of the process. Exported from a regular (non-_test.go) file —
// mirrors footer.DisableTimersForTesting (footer_testing.go) — so an
// importing package's test suite (e.g. internal/fuzz/harness) can silence
// it too; production code never calls this.
func DisableForTesting() {
	disabledForTesting = true
}

// NewClient constructs a Client from environment variables.
//
// Reads:
//   - OPENAI_API_KEY   — required when using the public OpenAI endpoint
//   - OPENAI_BASE_URL  — defaults to "https://api.openai.com/v1"
//   - OPENAI_MODEL     — defaults to "gpt-4o"
func NewClient() (Client, error) {
	if disabledForTesting {
		return Client{}, errors.New("ai: disabled for testing")
	}

	const defaultBaseURL = "https://api.openai.com/v1"
	const defaultModel = "gpt-4o"

	baseURL := os.Getenv("OPENAI_BASE_URL")
	if baseURL == "" {
		baseURL = defaultBaseURL
	}

	model := os.Getenv("OPENAI_MODEL")
	if model == "" {
		model = defaultModel
	}

	apiKey := os.Getenv("OPENAI_API_KEY")
	if apiKey == "" && baseURL == defaultBaseURL {
		return Client{}, fmt.Errorf("OPENAI_API_KEY is required when using the public OpenAI endpoint")
	}

	return Client{
		BaseURL: baseURL,
		APIKey:  apiKey,
		Model:   model,
	}, nil
}
