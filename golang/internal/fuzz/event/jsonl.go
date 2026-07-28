package event

import (
	"bufio"
	"encoding/json"
	"fmt"
	"os"
)

type eventJSON struct {
	Kind      string  `json:"kind"`
	KeyIndex  *uint16 `json:"keyIndex,omitempty"`
	Text      *string `json:"text,omitempty"`
	Width     *uint8  `json:"width,omitempty"`
	Height    *uint8  `json:"height,omitempty"`
	PaneIndex *uint8  `json:"paneIndex,omitempty"`
	PathIndex *uint8  `json:"pathIndex,omitempty"`
	WatchSub  *uint8  `json:"watchSub,omitempty"`
}

func kindToString(k Kind) string {
	switch k {
	case KindKey:
		return "key"
	case KindText:
		return "text"
	case KindPaste:
		return "paste"
	case KindResize:
		return "resize"
	case KindFocus:
		return "focus"
	case KindWatch:
		return "watch"
	case KindExternalWrite:
		return "externalWrite"
	default:
		return fmt.Sprintf("unknown%d", k)
	}
}

func stringToKind(s string) Kind {
	switch s {
	case "key":
		return KindKey
	case "text":
		return KindText
	case "paste":
		return KindPaste
	case "resize":
		return KindResize
	case "focus":
		return KindFocus
	case "watch":
		return KindWatch
	case "externalWrite":
		return KindExternalWrite
	default:
		return 255
	}
}

// LoadJSONL reads a newline-delimited JSON file and decodes it to []Event.
func LoadJSONL(path string) ([]Event, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, fmt.Errorf("open %q: %w", path, err)
	}
	defer f.Close()

	var events []Event
	scanner := bufio.NewScanner(f)
	for scanner.Scan() {
		line := scanner.Bytes()
		if len(line) == 0 {
			continue
		}
		var ej eventJSON
		if err := json.Unmarshal(line, &ej); err != nil {
			continue // skip malformed lines
		}
		ev := Event{Kind: stringToKind(ej.Kind)}
		if ej.KeyIndex != nil {
			ev.KeyIndex = *ej.KeyIndex
		}
		if ej.Text != nil {
			ev.Text = *ej.Text
		}
		if ej.Width != nil {
			ev.Width = *ej.Width
		}
		if ej.Height != nil {
			ev.Height = *ej.Height
		}
		if ej.PaneIndex != nil {
			ev.PaneIndex = *ej.PaneIndex
		}
		if ej.PathIndex != nil {
			ev.PathIndex = *ej.PathIndex
		}
		if ej.WatchSub != nil {
			ev.WatchSub = *ej.WatchSub
		}
		events = append(events, ev)
	}
	return events, scanner.Err()
}

// SaveJSONL writes events to a newline-delimited JSON file.
func SaveJSONL(path string, events []Event) error {
	f, err := os.Create(path)
	if err != nil {
		return fmt.Errorf("create %q: %w", path, err)
	}
	defer f.Close()

	enc := json.NewEncoder(f)
	for _, ev := range events {
		ej := eventJSON{Kind: kindToString(ev.Kind)}
		switch ev.Kind {
		case KindKey:
			ej.KeyIndex = &ev.KeyIndex
		case KindText, KindPaste:
			ej.Text = &ev.Text
		case KindResize:
			ej.Width = &ev.Width
			ej.Height = &ev.Height
		case KindFocus:
			ej.PaneIndex = &ev.PaneIndex
		case KindWatch:
			ej.PathIndex = &ev.PathIndex
			ej.WatchSub = &ev.WatchSub
		case KindExternalWrite:
			ej.PathIndex = &ev.PathIndex
			ej.Text = &ev.Text
		}
		if err := enc.Encode(ej); err != nil {
			return fmt.Errorf("encode event: %w", err)
		}
	}
	return nil
}
