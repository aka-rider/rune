package artifact

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"time"

	"rune/internal/fuzz/event"
	"rune/internal/fuzz/invariant"
	"rune/pkg/ui/components/textedit"
)

// Bundle represents a written artifact directory.
type Bundle struct {
	Dir string
}

// cellJSON is a JSON-serializable representation of textedit.Cell.
// lipgloss.Style is omitted because it is not JSON-serializable.
type cellJSON struct {
	Rune      string `json:"rune"`
	Width     int    `json:"width"`
	BufOffset int    `json:"bufOffset"`
	Selected  bool   `json:"selected,omitempty"`
	Cursor    bool   `json:"cursor,omitempty"`
}

// metaJSON holds terminal geometry recorded at violation time.
type metaJSON struct {
	InvariantID string `json:"invariantID"`
	Width       int    `json:"width"`
	Height      int    `json:"height"`
	// TimestampNs is left 0; the directory name already encodes the timestamp.
	TimestampNs int64 `json:"timestampNs"`
}

// Write creates a new finding directory under baseDir and writes:
//   - violation.json: the Violation struct as JSON
//   - frame.txt: the frozen terminal frame string
//   - keys.jsonl: the event sequence in JSONL format
//   - cells.json: serialized cell grid snapshot (Style field omitted)
//   - meta.json: terminal width/height and invariant ID
//   - screenshot.png: placeholder (empty file; Playwright fills this in Phase 0)
//
// Returns the Bundle with the directory path, or an error.
func Write(baseDir string, v *invariant.Violation, frame string, events []event.Event, cells [][]textedit.Cell, w, h int) (*Bundle, error) {
	dir := filepath.Join(baseDir, fmt.Sprintf("finding-%d", time.Now().UnixNano()))
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return nil, fmt.Errorf("create artifact dir %q: %w", dir, err)
	}

	// violation.json
	vjData, err := json.MarshalIndent(v, "", "  ")
	if err != nil {
		return nil, fmt.Errorf("marshal violation: %w", err)
	}
	if err := os.WriteFile(filepath.Join(dir, "violation.json"), vjData, 0o644); err != nil {
		return nil, fmt.Errorf("write violation.json: %w", err)
	}

	// frame.txt
	if err := os.WriteFile(filepath.Join(dir, "frame.txt"), []byte(frame), 0o644); err != nil {
		return nil, fmt.Errorf("write frame.txt: %w", err)
	}

	// keys.jsonl
	if err := event.SaveJSONL(filepath.Join(dir, "keys.jsonl"), events); err != nil {
		return nil, fmt.Errorf("write keys.jsonl: %w", err)
	}

	// cells.json — convert [][]textedit.Cell to [][]cellJSON (drop Style)
	grid := make([][]cellJSON, len(cells))
	for i, line := range cells {
		row := make([]cellJSON, len(line))
		for j, c := range line {
			r := string(c.Rune)
			if c.Grapheme != "" {
				r = c.Grapheme
			}
			row[j] = cellJSON{
				Rune:      r,
				Width:     c.Width,
				BufOffset: c.BufOffset,
				Selected:  c.Selected,
				Cursor:    c.Cursor,
			}
		}
		grid[i] = row
	}
	cellsData, err := json.MarshalIndent(grid, "", "  ")
	if err != nil {
		return nil, fmt.Errorf("marshal cells: %w", err)
	}
	if err := os.WriteFile(filepath.Join(dir, "cells.json"), cellsData, 0o644); err != nil {
		return nil, fmt.Errorf("write cells.json: %w", err)
	}

	// meta.json
	meta := metaJSON{
		InvariantID: v.InvariantID,
		Width:       w,
		Height:      h,
	}
	metaData, err := json.MarshalIndent(meta, "", "  ")
	if err != nil {
		return nil, fmt.Errorf("marshal meta: %w", err)
	}
	if err := os.WriteFile(filepath.Join(dir, "meta.json"), metaData, 0o644); err != nil {
		return nil, fmt.Errorf("write meta.json: %w", err)
	}

	// screenshot.png placeholder (Playwright will overwrite this)
	if err := os.WriteFile(filepath.Join(dir, "screenshot.png"), nil, 0o644); err != nil {
		return nil, fmt.Errorf("write screenshot.png placeholder: %w", err)
	}

	return &Bundle{Dir: dir}, nil
}
