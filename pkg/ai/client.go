package ai

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"time"
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

// httpClient is the shared transport for all API calls. Unlike
// http.DefaultClient it carries a hard timeout: a black-holed dial or a
// stalled response must fail and surface, never leave the chat pane
// "loading" forever waiting on a request only a resubmit would cancel.
var httpClient = &http.Client{Timeout: 60 * time.Second}

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

	resp, err := httpClient.Do(req)
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
