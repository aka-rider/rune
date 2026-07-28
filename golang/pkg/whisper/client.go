package whisper

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"mime/multipart"
	"net/http"
	"net/textproto"
)

// Client is a thin HTTP client for a whisper.cpp OpenAI-compatible transcription server.
type Client struct {
	BaseURL       string // e.g. "http://127.0.0.1:2022"
	InferencePath string // e.g. "/v1/audio/transcriptions"
}

// Transcribe sends wav audio to the server and returns the transcribed text.
// lang is an optional BCP-47 language code; pass "" to let the server auto-detect.
func (c Client) Transcribe(ctx context.Context, wav []byte, lang string) (string, error) {
	var body bytes.Buffer
	mw := multipart.NewWriter(&body)

	// file field with explicit content-type
	h := make(textproto.MIMEHeader)
	h.Set("Content-Disposition", `form-data; name="file"; filename="audio.wav"`)
	h.Set("Content-Type", "audio/wav")
	fw, err := mw.CreatePart(h)
	if err != nil {
		return "", fmt.Errorf("transcribe create part: %w", err)
	}
	if _, err := fw.Write(wav); err != nil {
		return "", fmt.Errorf("transcribe write wav: %w", err)
	}

	if lang != "" {
		if err := mw.WriteField("language", lang); err != nil {
			return "", fmt.Errorf("transcribe write language: %w", err)
		}
	}

	if err := mw.Close(); err != nil {
		return "", fmt.Errorf("transcribe close multipart: %w", err)
	}

	url := c.BaseURL + c.InferencePath
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, url, &body)
	if err != nil {
		return "", fmt.Errorf("transcribe create request: %w", err)
	}
	req.Header.Set("Content-Type", mw.FormDataContentType())

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return "", fmt.Errorf("transcribe request %q: %w", url, err)
	}
	defer resp.Body.Close() // fire-and-forget: response body cleanup

	respBytes, err := io.ReadAll(resp.Body)
	if err != nil {
		return "", fmt.Errorf("transcribe read response: %w", err)
	}

	if resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("transcribe: server %d: %s", resp.StatusCode, string(respBytes))
	}

	var result struct {
		Text string `json:"text"`
	}
	if err := json.Unmarshal(respBytes, &result); err != nil {
		return "", fmt.Errorf("transcribe parse response: %w", err)
	}

	return result.Text, nil
}
