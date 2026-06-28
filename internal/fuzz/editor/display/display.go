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

	// LINK-FOLD: a folded navigable link/wiki link must hide REAL delimiters and
	// render its label faithfully. The user-facing rule: a non-image link renders
	// as verbatim raw markdown (Revealed — skipped here, guarded by D5) OR as the
	// clean link name. Run over Syntax.Lines so each token is a whole, unsliced
	// span (wrap slicing would misalign BufferStart vs CellMap).
	for lineIdx, sline := range s.Syntax.Lines {
		for spanIdx, sp := range sline.Spans {
			if sp.State != display.Rendered || sp.CellMap == nil {
				continue
			}
			if sp.LinkRole() != display.LinkRoleNavigable {
				continue
			}
			// (a) The hidden prefix [BufferStart, firstCellOffset) must be a valid
			// opening delimiter for the kind. A wiki span whose start landed inside
			// the target (the BUG1 leak) has a prefix like "d|" — not "[[".
			firstOff := sp.CellMap[0].BufOffset
			if firstOff >= 0 && sp.BufferStart >= 0 && firstOff >= sp.BufferStart && firstOff <= len(s.Content) {
				prefix := s.Content[sp.BufferStart:firstOff]
				if !validLinkPrefix(sp.Kind, prefix) {
					return &invariant.Violation{
						InvariantID: "LINK-FOLD",
						Message: fmt.Sprintf(
							"syntax line %d span %d: folded link %q has invalid hidden prefix %q (kind %d)",
							lineIdx, spanIdx, trunc(sp.Text, 40), trunc(prefix, 20), sp.Kind,
						),
					}
				}
			}
			// (b) Each visible rune must equal the buffer rune its CellMap points at.
			ri := 0
			for _, tr := range sp.Text {
				if ri >= len(sp.CellMap) {
					break // length mismatch owned by D1
				}
				off := sp.CellMap[ri].BufOffset
				ri++
				if off < 0 {
					continue
				}
				if off >= len(s.Content) {
					return &invariant.Violation{
						InvariantID: "LINK-FOLD",
						Message: fmt.Sprintf(
							"syntax line %d span %d: link rune %q maps to offset %d out of range (len %d)",
							lineIdx, spanIdx, string(tr), off, len(s.Content),
						),
					}
				}
				br, _ := utf8.DecodeRuneInString(s.Content[off:])
				if br != tr {
					return &invariant.Violation{
						InvariantID: "LINK-FOLD",
						Message: fmt.Sprintf(
							"syntax line %d span %d: link rune %q != buffer[%d] %q (text %q)",
							lineIdx, spanIdx, string(tr), off, string(br), trunc(sp.Text, 40),
						),
					}
				}
			}
		}
	}

	// LINK-CLEAN: a folded navigable link must not be flanked by leaked wrapping
	// emphasis delimiters. **[x](y)** rendering as **x** (BUG3) leaves the bold
	// "**" as Revealed siblings on both sides of the folded link.
	for lineIdx, sline := range s.Syntax.Lines {
		for spanIdx, sp := range sline.Spans {
			if sp.State != display.Rendered || sp.LinkRole() != display.LinkRoleNavigable {
				continue
			}
			var left, right string
			if spanIdx > 0 {
				if p := sline.Spans[spanIdx-1]; p.State == display.Revealed {
					left = emphSuffix(p.Text)
				}
			}
			if spanIdx+1 < len(sline.Spans) {
				if nx := sline.Spans[spanIdx+1]; nx.State == display.Revealed {
					right = emphPrefix(nx.Text)
				}
			}
			if left != "" && left == right {
				return &invariant.Violation{
					InvariantID: "LINK-CLEAN",
					Message: fmt.Sprintf(
						"syntax line %d span %d: folded link %q flanked by leaked emphasis %q",
						lineIdx, spanIdx, trunc(sp.Text, 40), left,
					),
				}
			}
		}
	}

	return nil
}

// validLinkPrefix reports whether prefix is a syntactically valid opening
// delimiter for a folded navigable link of the given kind. Wrapping decorations
// (bold/italic/strike) may precede the opener, so leading emphasis chars are
// tolerated before the [ / [[.
func validLinkPrefix(kind display.TokenKind, prefix string) bool {
	p := strings.TrimLeft(prefix, "*_~")
	switch kind {
	case display.TokenWikiLink:
		return strings.HasPrefix(p, "[[") || strings.HasPrefix(p, "![[")
	case display.TokenLink:
		return strings.HasPrefix(p, "[")
	}
	return true
}

// emphPrefix returns the leading run of identical emphasis chars (* _ ~), or "".
func emphPrefix(s string) string {
	if s == "" {
		return ""
	}
	c := s[0]
	if c != '*' && c != '_' && c != '~' {
		return ""
	}
	i := 0
	for i < len(s) && s[i] == c {
		i++
	}
	return s[:i]
}

// emphSuffix returns the trailing run of identical emphasis chars (* _ ~), or "".
func emphSuffix(s string) string {
	if s == "" {
		return ""
	}
	c := s[len(s)-1]
	if c != '*' && c != '_' && c != '~' {
		return ""
	}
	i := len(s)
	for i > 0 && s[i-1] == c {
		i--
	}
	return s[i:]
}
