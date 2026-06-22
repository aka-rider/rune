//go:build fuzzing

// Package display contains invariant checkers for the display pipeline:
// D1–D6 (span properties), WRAP-RT (wrap↔syntax round-trip), and
// SPAN-COVER (span coverage of each syntax line).
package display

import (
	"fmt"
	"strings"
	"unicode/utf8"

	"rune/internal/fuzz/invariant"
	"rune/internal/fuzz/snapshot"
	"rune/pkg/editor/display"
)

func trunc(s string, n int) string { return invariant.Trunc(s, n) }

// Check runs all L0 display-pipeline invariants against s.
// Returns the first violation, or nil.
func Check(s snapshot.Snapshot) *invariant.Violation {
	// D1: for each Rendered span with non-nil CellMap: len(CellMap) == RuneCount(Text).
	for lineIdx, dline := range s.Display.Lines {
		for spanIdx, sp := range dline.Spans {
			if sp.State != display.Rendered || sp.CellMap == nil {
				continue
			}
			want := utf8.RuneCountInString(sp.Text)
			if len(sp.CellMap) != want {
				return &invariant.Violation{
					InvariantID: "D1",
					Message: fmt.Sprintf(
						"display line %d span %d: CellMap len %d != rune count %d (text %q)",
						lineIdx, spanIdx, len(sp.CellMap), want, trunc(sp.Text, 40),
					),
				}
			}
		}
	}

	// D2: a Rendered span with non-empty Text must not have a nil CellMap.
	for lineIdx, dline := range s.Display.Lines {
		for spanIdx, sp := range dline.Spans {
			if sp.State == display.Rendered && sp.Text != "" && sp.CellMap == nil {
				return &invariant.Violation{
					InvariantID: "D2",
					Message: fmt.Sprintf(
						"display line %d span %d: Rendered span %q has nil CellMap",
						lineIdx, spanIdx, trunc(sp.Text, 40),
					),
				}
			}
		}
	}

	// D3: every span must have BufferStart ≤ BufferEnd.
	for lineIdx, dline := range s.Display.Lines {
		for spanIdx, sp := range dline.Spans {
			if sp.BufferStart > sp.BufferEnd {
				return &invariant.Violation{
					InvariantID: "D3",
					Message: fmt.Sprintf(
						"display line %d span %d: BufferStart %d > BufferEnd %d",
						lineIdx, spanIdx, sp.BufferStart, sp.BufferEnd,
					),
				}
			}
		}
	}

	// D5: a Revealed span's Text must equal raw buffer bytes [BufferStart:BufferEnd].
	for lineIdx, dline := range s.Display.Lines {
		for spanIdx, sp := range dline.Spans {
			if sp.State != display.Revealed || sp.Text == "" {
				continue
			}
			if sp.BufferStart < 0 || sp.BufferEnd > len(s.Content) || sp.BufferStart > sp.BufferEnd {
				continue // bounds already caught by D3
			}
			want := s.Content[sp.BufferStart:sp.BufferEnd]
			if sp.Text != want {
				return &invariant.Violation{
					InvariantID: "D5",
					Message: fmt.Sprintf(
						"display line %d span %d: Revealed text %q != buffer[%d:%d] %q",
						lineIdx, spanIdx,
						trunc(sp.Text, 40), sp.BufferStart, sp.BufferEnd,
						trunc(want, 40),
					),
				}
			}
		}
	}

	// D6: len(Display.Lines) == Display.TotalRows (one DisplayLine per wrap row).
	if s.Display.TotalRows > 0 && len(s.Display.Lines) != s.Display.TotalRows {
		return &invariant.Violation{
			InvariantID: "D6",
			Message: fmt.Sprintf(
				"len(Display.Lines)=%d != Display.TotalRows=%d",
				len(s.Display.Lines), s.Display.TotalRows,
			),
		}
	}

	// WRAP-RT: per model line, concatenated WrapSegment span texts equal the syntax-line text.
	for lineIdx, sline := range s.Syntax.Lines {
		syntaxText := ""
		for _, sp := range sline.Spans {
			syntaxText += sp.Text
		}
		wrapText := ""
		for _, seg := range s.Wrap.Segments {
			if seg.ModelLine != lineIdx {
				continue
			}
			for _, sp := range seg.Spans {
				wrapText += sp.Text
			}
		}
		if wrapText != syntaxText {
			return &invariant.Violation{
				InvariantID: "WRAP-RT",
				Message: fmt.Sprintf(
					"model line %d: wrap segments text %q != syntax line text %q",
					lineIdx, trunc(wrapText, 60), trunc(syntaxText, 60),
				),
			}
		}
	}

	// SPAN-COVER: per syntax line, span [BufferStart,BufferEnd) tiles the line with no gap/overlap.
	for lineIdx, sline := range s.Syntax.Lines {
		if len(sline.Spans) == 0 {
			continue
		}
		// Compute expected line start from Content.
		lineStart := 0
		for i, ch := range s.Content {
			if lineIdx == 0 {
				break
			}
			if ch == '\n' {
				lineIdx--
				lineStart = i + 1
			}
		}
		// Find line end.
		lineEnd := strings.Index(s.Content[lineStart:], "\n")
		if lineEnd < 0 {
			lineEnd = len(s.Content)
		} else {
			lineEnd += lineStart
		}

		// Check coverage within [lineStart, lineEnd].
		pos := lineStart
		for spanIdx, sp := range sline.Spans {
			if sp.BufferStart < pos {
				return &invariant.Violation{
					InvariantID: "SPAN-COVER",
					Message: fmt.Sprintf(
						"syntax line span[%d]: BufferStart %d < expected %d (overlap)",
						spanIdx, sp.BufferStart, pos,
					),
				}
			}
			if sp.BufferStart > pos {
				return &invariant.Violation{
					InvariantID: "SPAN-COVER",
					Message: fmt.Sprintf(
						"syntax line span[%d]: gap at [%d, %d) in line [%d, %d)",
						spanIdx, pos, sp.BufferStart, lineStart, lineEnd,
					),
				}
			}
			pos = sp.BufferEnd
		}
	}

	return nil
}
