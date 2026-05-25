package editortest

import (
	"fmt"
	"slices"
	"strings"
)

// TestState represents the state of a text buffer for testing purposes.
type TestState struct {
	Content string
	Cursors []CursorState // sorted by offset
}

// CursorState represents a single cursor position and optional selection.
type CursorState struct {
	// Position is the cursor offset within Content.
	Position int
	// Anchor is the selection anchor offset.
	// Anchor == Position means no selection.
	Anchor int
}

// ParseState parses a notation string into a TestState.
//
// Notation format:
//
//	|cursor at this position
//	[forward selection text]
//	]backward selection text[
//	\| \[ \] escape the special characters
//
// Examples:
//
//	"hello|world"       -> Content: "helloworld", Cursor{Position: 5}
//	"hello[world"        -> Content: "helloworld", Cursor{Position: 10, Anchor: 5}
//	"]hello[world"       -> Content: "helloworld", Cursor{Position: 5, Anchor: 10}
//	"|a[b|c"             -> Content: "abc", Cursors at 0 and 3
func ParseState(notation string) (TestState, error) {
	if notation == "" {
		return TestState{Content: "", Cursors: []CursorState{}}, nil
	}

	// Phase 1: tokenize into segments
	type token struct {
		kind      string // "text", "cursor", "selFwd", "selBack"
		text      string // the actual text (without brackets)
		anchor    int    // for selections: position of the anchor
		cursorPos int    // position of the cursor in the resulting string
	}

	var tokens []token
	var curPos int // position in the *parsed* string (not the notation)

	i := 0
	for i < len(notation) {
		ch := notation[i]

		switch {
		case ch == '\\':
			// Escape sequence
			if i+1 >= len(notation) {
				return TestState{}, fmt.Errorf("notation: trailing backslash at end of string")
			}
			escaped := notation[i+1]
			switch escaped {
			case '|', '[', ']':
				tokens = append(tokens, token{kind: "text", text: string(escaped)})
				i += 2
				curPos++
			default:
				// Treat backslash as literal
				tokens = append(tokens, token{kind: "text", text: "\\"})
				i++
			}

		case ch == '|':
			tokens = append(tokens, token{kind: "cursor"})
			i++

		case ch == '[':
			// Forward selection: [text]
			// Find matching ] by tracking depth
			depth := 1
			j := i + 1
			for j < len(notation) && depth > 0 {
				switch notation[j] {
				case '\\':
					j += 2 // skip escaped char
				case '[':
					depth++
					j++
				case ']':
					depth--
					if depth == 0 {
						break
					}
					j++
				default:
					j++
				}
			}
			if depth != 0 {
				// No closing bracket found
				return TestState{}, fmt.Errorf("notation: unclosed '[' at position %d", i)
			}
			// j is now at the closing ']'
			// Extract inner text: notation[i+1:j] (j points to closing ])
			inner := extractText(notation[i+1:j])
			selLen := len([]rune(inner))
			// In forward selection: anchor is at start, cursor is at end
			anchorOffset := curPos
			cursorOffset := curPos + selLen
			tokens = append(tokens, token{
				kind:      "selFwd",
				text:      inner,
				anchor:    anchorOffset,
				cursorPos: cursorOffset,
			})
			i = j + 1
			curPos += selLen

		case ch == ']':
			// Backward selection: ]text[
			// Find matching [ by tracking depth
			depth := 1
			j := i + 1
			for j < len(notation) && depth > 0 {
				switch notation[j] {
				case '\\':
					j += 2
				case ']':
					depth++
					j++
				case '[':
					depth--
					if depth == 0 {
						break
					}
					j++
				default:
					j++
				}
			}
			if depth != 0 {
				// No opening bracket found — orphan ]
				return TestState{}, fmt.Errorf("notation: orphan ']' at position %d", i)
			}
			// j is now at the opening '['
			inner := extractText(notation[i+1:j])
			selLen := len([]rune(inner))
			// In backward selection: cursor is at start, anchor is at end
			cursorOffset := curPos
			anchorOffset := curPos + selLen
			tokens = append(tokens, token{
				kind:      "selBack",
				text:      inner,
				anchor:    anchorOffset,
				cursorPos: cursorOffset,
			})
			i = j + 1
			curPos += selLen

		default:
			tokens = append(tokens, token{kind: "text", text: string(ch)})
			i++
			curPos++
		}
	}

	// Phase 2: build content and cursors
	var content strings.Builder
	var cursors []CursorState

	for _, t := range tokens {
		switch t.kind {
		case "text":
			content.WriteString(t.text)
		case "cursor":
			cursors = append(cursors, CursorState{Position: curPos, Anchor: curPos})
		case "selFwd":
			content.WriteString(t.text)
			cursors = append(cursors, CursorState{Position: t.cursorPos, Anchor: t.anchor})
		case "selBack":
			content.WriteString(t.text)
			cursors = append(cursors, CursorState{Position: t.cursorPos, Anchor: t.anchor})
		}
	}

	if len(cursors) == 0 {
		return TestState{}, fmt.Errorf("notation: no cursor marker '|' found")
	}

	// Sort cursors by position
	slices.SortFunc(cursors, func(a, b CursorState) int {
		return a.Position - b.Position
	})

	return TestState{Content: content.String(), Cursors: cursors}, nil
}

// FormatState formats a TestState back into a notation string.
func FormatState(s TestState) string {
	if len(s.Cursors) == 0 {
		return s.Content
	}

	// Build a map: offset -> CursorState
	cursorMap := make(map[int]CursorState, len(s.Cursors))
	for _, c := range s.Cursors {
		cursorMap[c.Position] = c
	}

	var buf strings.Builder
	runes := []rune(s.Content)
	contentLen := len(runes)
	i := 0

	for i < contentLen {
		cs, hasCursor := cursorMap[i]

		if !hasCursor {
			// Check if this position is inside a selection range
			insideSelection := false
			for _, c := range s.Cursors {
				if c.Position != c.Anchor {
					start, end := minInt(c.Position, c.Anchor), maxInt(c.Position, c.Anchor)
					if i > start && i < end {
						insideSelection = true
						break
					}
				}
			}
			if insideSelection {
				buf.WriteRune(runes[i])
				i++
				continue
			}
			buf.WriteRune(runes[i])
			i++
			continue
		}

		// We have a cursor at position i
		if cs.Position == cs.Anchor {
			// Simple cursor, no selection
			buf.WriteString("|")
			i++
		} else {
			// Selection
			start, end := minInt(cs.Position, cs.Anchor), maxInt(cs.Position, cs.Anchor)
			var selText strings.Builder
			for k := start; k < end; k++ {
				selText.WriteRune(runes[k])
			}
			selStr := escapeText(selText.String())

			if cs.Position < cs.Anchor {
				// Backward selection: ]text[
				buf.WriteString("]")
				buf.WriteString(selStr)
				buf.WriteString("[")
				i = end
			} else {
				// Forward selection: [text]
				buf.WriteString("[")
				buf.WriteString(selStr)
				buf.WriteString("]")
				i = cs.Anchor
			}
		}
	}

	return buf.String()
}

// escapeText escapes special characters in selection text.
func escapeText(s string) string {
	var buf strings.Builder
	for _, r := range s {
		switch r {
		case '|', '[', ']':
			buf.WriteRune('\\')
			buf.WriteRune(r)
		default:
			buf.WriteRune(r)
		}
	}
	return buf.String()
}

// extractText extracts text from a notation segment, handling escape sequences.
func extractText(s string) string {
	var buf strings.Builder
	i := 0
	for i < len(s) {
		if s[i] == '\\' && i+1 < len(s) {
			buf.WriteByte(s[i+1])
			i += 2
		} else {
			buf.WriteByte(s[i])
			i++
		}
	}
	return buf.String()
}

func minInt(a, b int) int {
	if a < b {
		return a
	}
	return b
}

func maxInt(a, b int) int {
	if a > b {
		return a
	}
	return b
}
