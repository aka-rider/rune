package event

import "unicode/utf8"

// Decode decodes a binary fuzzer input to a sequence of Events.
// It is total: every byte string produces some (possibly empty) event list.
func Decode(data []byte) []Event {
	var events []Event
	i := 0
	for i < len(data) {
		kind := Kind(data[i])
		i++
		switch kind {
		case KindKey:
			if i+2 > len(data) {
				return events
			}
			idx := uint16(data[i])<<8 | uint16(data[i+1])
			i += 2
			events = append(events, Event{Kind: KindKey, KeyIndex: idx})
		case KindText, KindPaste:
			if i >= len(data) {
				return events
			}
			n := int(data[i])
			i++
			if i+n > len(data) {
				n = len(data) - i
			}
			text := sanitizeUTF8(data[i : i+n])
			i += n
			events = append(events, Event{Kind: kind, Text: text})
		case KindResize:
			if i+2 > len(data) {
				return events
			}
			w := clamp(int(data[i]), 20, 220)
			h := clamp(int(data[i+1]), 5, 80)
			i += 2
			events = append(events, Event{Kind: KindResize, Width: uint8(w), Height: uint8(h)})
		case KindFocus:
			if i >= len(data) {
				return events
			}
			events = append(events, Event{Kind: KindFocus, PaneIndex: data[i] % 5})
			i++
		default:
			// Unknown kind: skip (0-byte payload)
		}
	}
	return events
}

func sanitizeUTF8(b []byte) string {
	var out []byte
	for len(b) > 0 {
		r, size := utf8.DecodeRune(b)
		if r != utf8.RuneError {
			out = append(out, b[:size]...)
		}
		b = b[size:]
	}
	return string(out)
}

func clamp(v, lo, hi int) int {
	if v < lo {
		return lo
	}
	if v > hi {
		return hi
	}
	return v
}
