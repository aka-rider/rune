package ai

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
)

// Message is a single chat message in OpenAI-compatible format.
type Message struct {
	Role    string `json:"role"`
	Content string `json:"content"`
}

// Client holds connection parameters for an OpenAI-compatible API.
// It is a pure value type — no pointers, no context fields.
type Client struct {
	BaseURL string
	APIKey  string
	Model   string
}

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

// Chat sends messages to the API and returns the assistant reply.
// The provided ctx is used for request cancellation.
func (c Client) Chat(ctx context.Context, messages []Message) (string, error) {
	reqBody := struct {
		Model    string    `json:"model"`
		Messages []Message `json:"messages"`
	}{
		Model:    c.Model,
		Messages: messages,
	}

	encoded, err := json.Marshal(reqBody)
	if err != nil {
		return "", fmt.Errorf("ai: marshal request: %w", err)
	}

	req, err := http.NewRequestWithContext(ctx, http.MethodPost, c.BaseURL+"/chat/completions", bytes.NewReader(encoded))
	if err != nil {
		return "", fmt.Errorf("ai: build request: %w", err)
	}
	req.Header.Set("Content-Type", "application/json")
	if c.APIKey != "" {
		req.Header.Set("Authorization", "Bearer "+c.APIKey)
	}

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return "", fmt.Errorf("ai: http: %w", err)
	}
	defer resp.Body.Close()

	var payload struct {
		Error *struct {
			Message string `json:"message"`
		} `json:"error"`
		Choices []struct {
			Message struct {
				Content string `json:"content"`
			} `json:"message"`
		} `json:"choices"`
	}

	if err := json.NewDecoder(resp.Body).Decode(&payload); err != nil {
		return "", fmt.Errorf("ai: decode: %w", err)
	}

	if payload.Error != nil && payload.Error.Message != "" {
		return "", fmt.Errorf("ai: API error: %s", payload.Error.Message)
	}

	if len(payload.Choices) == 0 {
		return "", fmt.Errorf("ai: no choices in response")
	}

	return payload.Choices[0].Message.Content, nil
}
