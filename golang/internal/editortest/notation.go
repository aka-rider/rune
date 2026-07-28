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

func ParseState(notation string) (TestState, error) {
	if notation == "" {
		return TestState{}, fmt.Errorf("notation: empty notation")
	}

	var content strings.Builder
	var cursors []CursorState

	var openForward []int
	var openBackward []int

	i := 0
	curPos := 0

	for i < len(notation) {
		ch := notation[i]

		if ch == '\\' {
			if i+1 >= len(notation) {
				return TestState{}, fmt.Errorf("notation: trailing backslash at end of string")
			}
			escaped := notation[i+1]
			if escaped == '|' || escaped == '[' || escaped == ']' || escaped == '\\' {
				content.WriteByte(escaped)
				curPos++
				i += 2
			} else {
				content.WriteByte('\\')
				curPos++
				i++
			}
			continue
		}

		if ch == '|' {
			cursors = append(cursors, CursorState{Position: curPos, Anchor: curPos})
			i++
			continue
		}

		if ch == '[' {
			if len(openBackward) > 0 {
				pos := openBackward[len(openBackward)-1]
				openBackward = openBackward[:len(openBackward)-1]
				cursors = append(cursors, CursorState{Position: pos, Anchor: curPos})
			} else {
				openForward = append(openForward, curPos)
			}
			i++
			continue
		}

		if ch == ']' {
			if len(openForward) > 0 {
				anchor := openForward[len(openForward)-1]
				openForward = openForward[:len(openForward)-1]
				cursors = append(cursors, CursorState{Position: curPos, Anchor: anchor})
			} else {
				openBackward = append(openBackward, curPos)
			}
			i++
			continue
		}

		content.WriteByte(ch)
		curPos++
		i++
	}

	if len(openForward) > 0 {
		return TestState{}, fmt.Errorf("notation: unclosed '['")
	}
	if len(openBackward) > 0 {
		return TestState{}, fmt.Errorf("notation: orphan ']'")
	}
	if len(cursors) == 0 {
		return TestState{}, fmt.Errorf("notation: no cursor marker '|' found")
	}

	slices.SortFunc(cursors, func(a, b CursorState) int {
		if a.Position != b.Position {
			return a.Position - b.Position
		}
		return a.Anchor - b.Anchor
	})

	return TestState{
		Content: content.String(),
		Cursors: cursors,
	}, nil
}

func FormatState(s TestState) string {
	if len(s.Cursors) == 0 {
		return s.Content
	}

	type event struct {
		pos   int
		ch    rune
		order int
	}
	var evs []event

	for _, c := range s.Cursors {
		if c.Position == c.Anchor {
			evs = append(evs, event{c.Position, '|', 5})
		} else if c.Anchor < c.Position {
			// forward
			evs = append(evs, event{c.Anchor, '[', 3})
			evs = append(evs, event{c.Position, ']', 1})
		} else {
			// backward
			evs = append(evs, event{c.Position, ']', 4})
			evs = append(evs, event{c.Anchor, '[', 2})
		}
	}

	// Sort events:
	// primary: pos ascending
	// secondary: order ascending
	// Wait, if order is same, preserve stability or something.
	// It's mostly unique except overlapping identical cursors.
	slices.SortFunc(evs, func(a, b event) int {
		if a.pos != b.pos {
			return a.pos - b.pos
		}
		return a.order - b.order
	})

	var buf strings.Builder
	evIdx := 0

	maxPos := len(s.Content)
	if len(evs) > 0 && evs[len(evs)-1].pos > maxPos {
		maxPos = evs[len(evs)-1].pos
	}

	for i := 0; i <= maxPos; i++ {
		for evIdx < len(evs) && evs[evIdx].pos == i {
			buf.WriteRune(evs[evIdx].ch)
			evIdx++
		}
		if i < len(s.Content) {
			ch := s.Content[i]
			if ch == '|' || ch == '[' || ch == ']' || ch == '\\' {
				buf.WriteByte('\\')
			}
			buf.WriteByte(ch)
		}
	}

	return buf.String()
}
