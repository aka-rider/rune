//go:build !fuzzing

package ai

import (
	"fmt"
	"os"
)

// NewClient constructs a Client from environment variables.
//
// Reads:
//   - OPENAI_API_KEY   — required when using the public OpenAI endpoint
//   - OPENAI_BASE_URL  — defaults to "https://api.openai.com/v1"
//   - OPENAI_MODEL     — defaults to "gpt-4o"
func NewClient() (Client, error) {
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
