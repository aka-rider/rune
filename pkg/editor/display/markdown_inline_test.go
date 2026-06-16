package display_test

import (
	"strings"
	"testing"

	"rune/pkg/editor/buffer"
	"rune/pkg/editor/coords"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/display"
)

// ==========================================================================
// Gate 1: Parser source ranges are byte-accurate for every WP14A element
// ==========================================================================

func TestSyntaxMap_HeadingSourceRange(t *testing.T) {
	text := "## Hello"
	buf := buffer.New(text)
	_ = buf
	sMap := display.NewSyntaxMap()

	// Cursor NOT on line → rendered (hides delimiters)
	cursorsOff := cursor.NewCursorSet(0)
	_, snap := sMap.Sync(buffer.New("other\n## Hello"), cursorsOff)

	line := snap.Lines[1]
	if len(line.Spans) == 0 {
		t.Fatal("expected spans on heading line")
	}
	found := false
	for _, sp := range line.Spans {
		if sp.Kind == display.TokenHeading {
			found = true
			if sp.Text != "Hello" {
				t.Errorf("heading text: got %q, want %q", sp.Text, "Hello")
			}
			if sp.State != display.Rendered {
				t.Errorf("heading state: got %v, want Rendered", sp.State)
			}
		}
	}
	if !found {
		t.Error("no heading span found")
	}

	// Cursor ON line → revealed
	cursorOnLine := cursor.NewCursorSet(len("other\n"))
	_, snap2 := sMap.Sync(buffer.New("other\n## Hello"), cursorOnLine)
	line2 := snap2.Lines[1]
	found2 := false
	for _, sp := range line2.Spans {
		if sp.Kind == display.TokenHeading {
			found2 = true
			if sp.Text != "## Hello" {
				t.Errorf("revealed heading text: got %q, want %q", sp.Text, "## Hello")
			}
			if sp.State != display.Revealed {
				t.Errorf("heading state: got %v, want Revealed", sp.State)
			}
		}
	}
	if !found2 {
		t.Error("no heading span found when revealed")
	}

	// Source range check with single-line buffer
	_, snapSingle := sMap.Sync(buf, cursor.NewCursorSet(5)) // cursor inside heading
	line0 := snapSingle.Lines[0]
	for _, sp := range line0.Spans {
		if sp.Kind == display.TokenHeading {
			// BufferStart should be 0, BufferEnd should be 8
			if sp.BufferStart != 0 {
				t.Errorf("heading BufferStart: got %d, want 0", sp.BufferStart)
			}
			if sp.BufferEnd != len(text) {
				t.Errorf("heading BufferEnd: got %d, want %d", sp.BufferEnd, len(text))
			}
		}
	}
}

func TestSyntaxMap_BoldSourceRange(t *testing.T) {
	text := "hello **bold** world"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor outside bold
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))
	line := snap.Lines[0]
	var boldSpan *display.SyntaxSpan
	for i := range line.Spans {
		if line.Spans[i].Kind == display.TokenBold {
			boldSpan = &line.Spans[i]
			break
		}
	}
	if boldSpan == nil {
		t.Fatal("no bold span found")
	}
	if boldSpan.Text != "bold" {
		t.Errorf("bold text: got %q, want %q", boldSpan.Text, "bold")
	}
	if boldSpan.State != display.Rendered {
		t.Errorf("bold state: got %v, want Rendered", boldSpan.State)
	}
	// **bold** starts at byte 6, ends at byte 14
	if boldSpan.BufferStart != 6 {
		t.Errorf("bold BufferStart: got %d, want 6", boldSpan.BufferStart)
	}
	if boldSpan.BufferEnd != 14 {
		t.Errorf("bold BufferEnd: got %d, want 14", boldSpan.BufferEnd)
	}
}

func TestSyntaxMap_ItalicSourceRange(t *testing.T) {
	text := "hello *italic* world"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))
	line := snap.Lines[0]
	var italicSpan *display.SyntaxSpan
	for i := range line.Spans {
		if line.Spans[i].Kind == display.TokenItalic {
			italicSpan = &line.Spans[i]
			break
		}
	}
	if italicSpan == nil {
		t.Fatal("no italic span found")
	}
	if italicSpan.Text != "italic" {
		t.Errorf("italic text: got %q, want %q", italicSpan.Text, "italic")
	}
	if italicSpan.State != display.Rendered {
		t.Errorf("italic state: got %v, want Rendered", italicSpan.State)
	}
	// *italic* starts at byte 6, ends at byte 14
	if italicSpan.BufferStart != 6 {
		t.Errorf("italic BufferStart: got %d, want 6", italicSpan.BufferStart)
	}
	if italicSpan.BufferEnd != 14 {
		t.Errorf("italic BufferEnd: got %d, want 14", italicSpan.BufferEnd)
	}
}

func TestSyntaxMap_StrikethroughSourceRange(t *testing.T) {
	text := "hello ~~strike~~ world"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))
	line := snap.Lines[0]
	var span *display.SyntaxSpan
	for i := range line.Spans {
		if line.Spans[i].Kind == display.TokenStrikethrough {
			span = &line.Spans[i]
			break
		}
	}
	if span == nil {
		t.Fatal("no strikethrough span found")
	}
	if span.Text != "strike" {
		t.Errorf("strikethrough text: got %q, want %q", span.Text, "strike")
	}
	if span.State != display.Rendered {
		t.Errorf("state: got %v, want Rendered", span.State)
	}
	// ~~strike~~ starts at byte 6, ends at byte 16
	if span.BufferStart != 6 {
		t.Errorf("BufferStart: got %d, want 6", span.BufferStart)
	}
	if span.BufferEnd != 16 {
		t.Errorf("BufferEnd: got %d, want 16", span.BufferEnd)
	}
}

func TestSyntaxMap_InlineCodeSourceRange(t *testing.T) {
	text := "hello `code` world"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))
	line := snap.Lines[0]
	var span *display.SyntaxSpan
	for i := range line.Spans {
		if line.Spans[i].Kind == display.TokenInlineCode {
			span = &line.Spans[i]
			break
		}
	}
	if span == nil {
		t.Fatal("no inline code span found")
	}
	if span.Text != "code" {
		t.Errorf("code text: got %q, want %q", span.Text, "code")
	}
	if span.State != display.Rendered {
		t.Errorf("state: got %v, want Rendered", span.State)
	}
	// `code` starts at byte 6, ends at byte 12
	if span.BufferStart != 6 {
		t.Errorf("BufferStart: got %d, want 6", span.BufferStart)
	}
	if span.BufferEnd != 12 {
		t.Errorf("BufferEnd: got %d, want 12", span.BufferEnd)
	}
}

func TestSyntaxMap_LinkSourceRange(t *testing.T) {
	text := "see [link](http://x.com) here"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))
	line := snap.Lines[0]
	var span *display.SyntaxSpan
	for i := range line.Spans {
		if line.Spans[i].Kind == display.TokenLink {
			span = &line.Spans[i]
			break
		}
	}
	if span == nil {
		t.Fatal("no link span found")
	}
	if span.Text != "link" {
		t.Errorf("link text: got %q, want %q", span.Text, "link")
	}
	if span.State != display.Rendered {
		t.Errorf("state: got %v, want Rendered", span.State)
	}
	// [link](http://x.com) starts at byte 4, ends at byte 24
	if span.BufferStart != 4 {
		t.Errorf("BufferStart: got %d, want 4", span.BufferStart)
	}
	if span.BufferEnd != 24 {
		t.Errorf("BufferEnd: got %d, want 24", span.BufferEnd)
	}
}

func TestSyntaxMap_BlockquoteSourceRange(t *testing.T) {
	text := "> quoted text"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor NOT on line
	_, snap := sMap.Sync(buffer.New("other\n> quoted text"), cursor.NewCursorSet(0))
	line := snap.Lines[1]
	var span *display.SyntaxSpan
	for i := range line.Spans {
		if line.Spans[i].Kind == display.TokenBlockquote {
			span = &line.Spans[i]
			break
		}
	}
	if span == nil {
		t.Fatal("no blockquote span found")
	}
	if span.Text != "quoted text" {
		t.Errorf("blockquote text: got %q, want %q", span.Text, "quoted text")
	}
	if span.State != display.Rendered {
		t.Errorf("state: got %v, want Rendered", span.State)
	}

	// Single-line check for buffer ranges
	_, snapSingle := sMap.Sync(buf, cursor.NewCursorSet(len(text))) // cursor past end
	line0 := snapSingle.Lines[0]
	for i := range line0.Spans {
		if line0.Spans[i].Kind == display.TokenBlockquote {
			if line0.Spans[i].BufferStart != 0 {
				t.Errorf("BufferStart: got %d, want 0", line0.Spans[i].BufferStart)
			}
			if line0.Spans[i].BufferEnd != len(text) {
				t.Errorf("BufferEnd: got %d, want %d", line0.Spans[i].BufferEnd, len(text))
			}
		}
	}
}

func TestSyntaxMap_TaskListSourceRange(t *testing.T) {
	text := "- [ ] todo item"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor off-line (blank line needed for goldmark to parse list correctly)
	_, snap := sMap.Sync(buffer.New("other\n\n- [ ] todo item"), cursor.NewCursorSet(0))
	line := snap.Lines[2]
	var span *display.SyntaxSpan
	for i := range line.Spans {
		if line.Spans[i].Kind == display.TokenTaskList {
			span = &line.Spans[i]
			break
		}
	}
	if span == nil {
		t.Fatal("no task list span found")
	}
	if span.Text != "☐ " {
		t.Errorf("task text: got %q, want %q", span.Text, "☐ ")
	}
	if span.State != display.Rendered {
		t.Errorf("state: got %v, want Rendered", span.State)
	}

	// Checked task
	_, snapChecked := sMap.Sync(buffer.New("other\n\n- [x] done item"), cursor.NewCursorSet(0))
	lineChecked := snapChecked.Lines[2]
	for i := range lineChecked.Spans {
		if lineChecked.Spans[i].Kind == display.TokenTaskList {
			if lineChecked.Spans[i].Text != "☑ " {
				t.Errorf("checked task text: got %q, want %q", lineChecked.Spans[i].Text, "☑ ")
			}
		}
	}

	// Buffer ranges — single line
	_, snapSingle := sMap.Sync(buf, cursor.NewCursorSet(len(text)))
	for i := range snapSingle.Lines[0].Spans {
		sp := snapSingle.Lines[0].Spans[i]
		if sp.Kind == display.TokenTaskList {
			if sp.BufferEnd < sp.BufferStart {
				t.Errorf("BufferEnd < BufferStart")
			}
		}
	}
}

func TestSyntaxMap_HorizontalRuleSourceRange(t *testing.T) {
	text := "---"
	sMap := display.NewSyntaxMap()

	// Cursor NOT on line (blank line needed so --- is not a setext heading)
	_, snap := sMap.Sync(buffer.New("other\n\n---"), cursor.NewCursorSet(0))
	line := snap.Lines[2]
	var span *display.SyntaxSpan
	for i := range line.Spans {
		if line.Spans[i].Kind == display.TokenHorizontalRule {
			span = &line.Spans[i]
			break
		}
	}
	if span == nil {
		t.Fatal("no horizontal rule span found")
	}
	if span.Text != "───" {
		t.Errorf("hr text: got %q, want %q", span.Text, "───")
	}
	if span.State != display.Rendered {
		t.Errorf("state: got %v, want Rendered", span.State)
	}

	// Buffer ranges
	buf := buffer.New(text)
	_, snapSingle := sMap.Sync(buf, cursor.NewCursorSet(len(text)))
	for i := range snapSingle.Lines[0].Spans {
		sp := snapSingle.Lines[0].Spans[i]
		if sp.Kind == display.TokenHorizontalRule {
			if sp.BufferStart != 0 {
				t.Errorf("BufferStart: got %d, want 0", sp.BufferStart)
			}
			if sp.BufferEnd != 3 {
				t.Errorf("BufferEnd: got %d, want 3", sp.BufferEnd)
			}
		}
	}
}

// ==========================================================================
// Gate 2: Offset deltas are sorted and monotonic
// ==========================================================================

func TestSyntaxMap_DeltasSortedMonotonic(t *testing.T) {
	tests := []struct {
		name string
		text string
	}{
		{"bold", "hello **bold** world"},
		{"italic", "one *two* three"},
		{"multiple", "**a** *b* ~~c~~ `d`"},
		{"heading", "## title"},
		{"link", "[text](url)"},
		{"mixed", "# heading\n**bold** and *italic*\n> quote"},
	}

	sMap := display.NewSyntaxMap()
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			buf := buffer.New(tc.text)
			_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))

			// Check per-line deltas
			for lineIdx := 0; lineIdx < buf.LineCount(); lineIdx++ {
				bp := coords.BufferPoint{Line: lineIdx, Col: 0}
				sp := snap.BufferToSyntax(bp) // exercise the path
				_ = sp

				// Check global deltas are monotonically non-decreasing in BufferOffset
				prev := -1
				for _, d := range snap.Deltas {
					if d.BufferOffset < prev {
						t.Errorf("global deltas not sorted: offset %d after %d", d.BufferOffset, prev)
					}
					prev = d.BufferOffset
				}
			}

			// Verify per-line delta values are monotonically non-decreasing.
			// Global deltas reset per-line, so we verify monotonicity within
			// contiguous runs of the same line (BufferOffset within a line's range).
			for lineIdx := 0; lineIdx < buf.LineCount(); lineIdx++ {
				lineStart := buf.LineStart(lineIdx)
				var lineEnd int
				if lineIdx < buf.LineCount()-1 {
					lineEnd = buf.LineStart(lineIdx + 1)
				} else {
					lineEnd = buf.Len() + 1
				}
				prevDelta := 0
				for _, d := range snap.Deltas {
					if d.BufferOffset >= lineStart && d.BufferOffset < lineEnd {
						if d.Delta < prevDelta {
							t.Errorf("line %d: delta at offset %d has Delta=%d < prev=%d — not monotonic",
								lineIdx, d.BufferOffset, d.Delta, prevDelta)
						}
						prevDelta = d.Delta
					}
				}
			}
		})
	}
}

// ==========================================================================
// Gate 3: Cursor inside token reveals only that token, not siblings
// ==========================================================================

func TestMarkdownInline_RevealIsolation(t *testing.T) {
	text := "hello **bold** and *italic* end"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Place cursor inside **bold** (at byte 8, within "bold")
	cursorInBold := cursor.NewCursorSet(8)
	_, snap := sMap.Sync(buf, cursorInBold)

	line := snap.Lines[0]
	var boldSpan, italicSpan *display.SyntaxSpan
	for i := range line.Spans {
		switch line.Spans[i].Kind {
		case display.TokenBold:
			boldSpan = &line.Spans[i]
		case display.TokenItalic:
			italicSpan = &line.Spans[i]
		}
	}

	if boldSpan == nil {
		t.Fatal("no bold span")
	}
	if italicSpan == nil {
		t.Fatal("no italic span")
	}

	// Bold should be revealed (cursor inside)
	if boldSpan.State != display.Revealed {
		t.Errorf("bold should be Revealed, got %v", boldSpan.State)
	}
	// Italic should remain rendered (cursor NOT inside)
	if italicSpan.State != display.Rendered {
		t.Errorf("italic should be Rendered, got %v", italicSpan.State)
	}

	// Now place cursor inside *italic* (at byte 20, within "italic")
	cursorInItalic := cursor.NewCursorSet(20)
	_, snap2 := sMap.Sync(buf, cursorInItalic)

	line2 := snap2.Lines[0]
	var boldSpan2, italicSpan2 *display.SyntaxSpan
	for i := range line2.Spans {
		switch line2.Spans[i].Kind {
		case display.TokenBold:
			boldSpan2 = &line2.Spans[i]
		case display.TokenItalic:
			italicSpan2 = &line2.Spans[i]
		}
	}

	if boldSpan2 == nil {
		t.Fatal("no bold span in snap2")
	}
	if italicSpan2 == nil {
		t.Fatal("no italic span in snap2")
	}

	// Bold should be rendered (cursor NOT inside)
	if boldSpan2.State != display.Rendered {
		t.Errorf("bold should be Rendered, got %v", boldSpan2.State)
	}
	// Italic should be revealed (cursor inside)
	if italicSpan2.State != display.Revealed {
		t.Errorf("italic should be Revealed, got %v", italicSpan2.State)
	}
}

// ==========================================================================
// Gate 4: Line-level reveal reveals only the cursor line
// ==========================================================================

func TestMarkdownLine_RevealOnlyCursorLine(t *testing.T) {
	text := "## Heading One\n## Heading Two\n## Heading Three"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Place cursor on line 1 (start of "## Heading Two")
	offset := len("## Heading One\n")
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(offset))

	// Line 0 should be rendered (delimiters hidden)
	for _, sp := range snap.Lines[0].Spans {
		if sp.Kind == display.TokenHeading && sp.State != display.Rendered {
			t.Errorf("line 0 heading should be Rendered, got %v", sp.State)
		}
	}
	// Line 1 should be revealed
	for _, sp := range snap.Lines[1].Spans {
		if sp.Kind == display.TokenHeading && sp.State != display.Revealed {
			t.Errorf("line 1 heading should be Revealed, got %v", sp.State)
		}
	}
	// Line 2 should be rendered
	for _, sp := range snap.Lines[2].Spans {
		if sp.Kind == display.TokenHeading && sp.State != display.Rendered {
			t.Errorf("line 2 heading should be Rendered, got %v", sp.State)
		}
	}
}

func TestMarkdownLine_BlockquoteRevealOnlyCursorLine(t *testing.T) {
	text := "> first quote\n> second quote\n> third quote"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor on line 1
	offset := len("> first quote\n")
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(offset))

	for _, sp := range snap.Lines[0].Spans {
		if sp.Kind == display.TokenBlockquote && sp.State != display.Rendered {
			t.Errorf("line 0 blockquote should be Rendered, got %v", sp.State)
		}
	}
	for _, sp := range snap.Lines[1].Spans {
		if sp.Kind == display.TokenBlockquote && sp.State != display.Revealed {
			t.Errorf("line 1 blockquote should be Revealed, got %v", sp.State)
		}
	}
	for _, sp := range snap.Lines[2].Spans {
		if sp.Kind == display.TokenBlockquote && sp.State != display.Rendered {
			t.Errorf("line 2 blockquote should be Rendered, got %v", sp.State)
		}
	}
}

// ==========================================================================
// Coordinate round-trips for cursor-legal positions
// ==========================================================================

func TestSyntaxMap_CoordinateRoundTrip(t *testing.T) {
	tests := []struct {
		name string
		text string
	}{
		{"bold", "hello **bold** world"},
		{"italic", "one *two* three"},
		{"multiple", "**a** *b* ~~c~~ `d`"},
		{"link", "see [link](http://x.com) here"},
		{"heading", "## title"},
		{"mixed_lines", "# h1\n**bold** text\n> quote\n---"},
	}

	sMap := display.NewSyntaxMap()
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			buf := buffer.New(tc.text)
			_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))

			for line := 0; line < buf.LineCount(); line++ {
				lineText := buf.Line(line)
				for col := 0; col <= len(lineText); col++ {
					bp := coords.BufferPoint{Line: line, Col: col}
					sp := snap.BufferToSyntax(bp)
					bp2 := snap.SyntaxToBuffer(sp)

					// Stability: BufferToSyntax(bp2) == sp
					sp2 := snap.BufferToSyntax(bp2)
					if sp != sp2 {
						t.Errorf("stability violation: bp=%v → sp=%v → bp2=%v → sp2=%v",
							bp, sp, bp2, sp2)
					}

					// If bp is cursor-legal, round-trip must be identity
					if bp == bp2 {
						// Good — cursor-legal position round-trips
					} else {
						// bp is inside hidden delimiter; verify bp2 is stable
						bp3 := snap.SyntaxToBuffer(sp2)
						if bp2 != bp3 {
							t.Errorf("clamped position not stable: bp2=%v → sp2=%v → bp3=%v",
								bp2, sp2, bp3)
						}
					}
				}
			}
		})
	}
}

// ==========================================================================
// Hidden-delimiter clamp tests
// ==========================================================================

func TestSyntaxMap_HiddenDelimiterClamp(t *testing.T) {
	// "hello **bold** world"
	// ** is at cols 6,7 (left delim) and 12,13 (right delim)
	text := "hello **bold** world"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor far away so bold is Rendered (delimiters hidden)
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))

	// Col 6 is inside left delimiter "**" → should clamp to col 8
	bp6 := coords.BufferPoint{Line: 0, Col: 6}
	sp6 := snap.BufferToSyntax(bp6)
	bp6rt := snap.SyntaxToBuffer(sp6)
	if bp6rt.Col != 8 {
		t.Errorf("col 6 should clamp to buffer col 8, got %d", bp6rt.Col)
	}

	// Col 7 is also inside "**" → same clamp
	bp7 := coords.BufferPoint{Line: 0, Col: 7}
	sp7 := snap.BufferToSyntax(bp7)
	bp7rt := snap.SyntaxToBuffer(sp7)
	if bp7rt.Col != 8 {
		t.Errorf("col 7 should clamp to buffer col 8, got %d", bp7rt.Col)
	}

	// Col 8 is first char of "bold" — cursor-legal, should round-trip
	bp8 := coords.BufferPoint{Line: 0, Col: 8}
	sp8 := snap.BufferToSyntax(bp8)
	bp8rt := snap.SyntaxToBuffer(sp8)
	if bp8rt != bp8 {
		t.Errorf("col 8 should round-trip, got %v", bp8rt)
	}

	// Col 12 is start of right "**" → should clamp to col 14
	bp12 := coords.BufferPoint{Line: 0, Col: 12}
	sp12 := snap.BufferToSyntax(bp12)
	bp12rt := snap.SyntaxToBuffer(sp12)
	if bp12rt.Col != 14 {
		t.Errorf("col 12 should clamp to buffer col 14, got %d", bp12rt.Col)
	}

	// Col 14 is 'w' in "world" — cursor-legal
	bp14 := coords.BufferPoint{Line: 0, Col: 14}
	sp14 := snap.BufferToSyntax(bp14)
	bp14rt := snap.SyntaxToBuffer(sp14)
	if bp14rt != bp14 {
		t.Errorf("col 14 should round-trip, got %v", bp14rt)
	}
}

func TestSyntaxMap_HiddenDelimiterClamp_Italic(t *testing.T) {
	// "one *two* three"
	// * is at col 4 (left) and col 8 (right)
	text := "one *two* three"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))

	// Col 4 inside left * → clamp to 5
	bp4 := coords.BufferPoint{Line: 0, Col: 4}
	sp4 := snap.BufferToSyntax(bp4)
	bp4rt := snap.SyntaxToBuffer(sp4)
	if bp4rt.Col != 5 {
		t.Errorf("col 4 should clamp to 5, got %d", bp4rt.Col)
	}

	// Col 5 is 't' of "two" — cursor-legal
	bp5 := coords.BufferPoint{Line: 0, Col: 5}
	sp5 := snap.BufferToSyntax(bp5)
	bp5rt := snap.SyntaxToBuffer(sp5)
	if bp5rt != bp5 {
		t.Errorf("col 5 should round-trip, got %v", bp5rt)
	}

	// Col 8 inside right * → clamp to 9
	bp8 := coords.BufferPoint{Line: 0, Col: 8}
	sp8 := snap.BufferToSyntax(bp8)
	bp8rt := snap.SyntaxToBuffer(sp8)
	if bp8rt.Col != 9 {
		t.Errorf("col 8 should clamp to 9, got %d", bp8rt.Col)
	}
}

// ==========================================================================
// Reveal transition: cursor enters and exits bold without buffer offset change
// ==========================================================================

func TestMarkdownInline_RevealTransition(t *testing.T) {
	text := "hello **bold** world"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor at col 5 (the space before **) — outside
	_, snapOutside := sMap.Sync(buf, cursor.NewCursorSet(5))
	lineOutside := snapOutside.Lines[0]
	var boldOutside *display.SyntaxSpan
	for i := range lineOutside.Spans {
		if lineOutside.Spans[i].Kind == display.TokenBold {
			boldOutside = &lineOutside.Spans[i]
			break
		}
	}
	if boldOutside == nil {
		t.Fatal("no bold span when cursor outside")
	}
	if boldOutside.State != display.Rendered {
		t.Errorf("bold should be Rendered when cursor outside, got %v", boldOutside.State)
	}
	if boldOutside.Text != "bold" {
		t.Errorf("rendered bold text should be 'bold', got %q", boldOutside.Text)
	}

	// Cursor at col 8 (first char of "bold") — inside
	_, snapInside := sMap.Sync(buf, cursor.NewCursorSet(8))
	lineInside := snapInside.Lines[0]
	var boldInside *display.SyntaxSpan
	for i := range lineInside.Spans {
		if lineInside.Spans[i].Kind == display.TokenBold {
			boldInside = &lineInside.Spans[i]
			break
		}
	}
	if boldInside == nil {
		t.Fatal("no bold span when cursor inside")
	}
	if boldInside.State != display.Revealed {
		t.Errorf("bold should be Revealed when cursor inside, got %v", boldInside.State)
	}
	if boldInside.Text != "**bold**" {
		t.Errorf("revealed bold text should be '**bold**', got %q", boldInside.Text)
	}

	// Cursor at col 14 (first char after **) — outside again
	_, snapAfter := sMap.Sync(buf, cursor.NewCursorSet(14))
	lineAfter := snapAfter.Lines[0]
	var boldAfter *display.SyntaxSpan
	for i := range lineAfter.Spans {
		if lineAfter.Spans[i].Kind == display.TokenBold {
			boldAfter = &lineAfter.Spans[i]
			break
		}
	}
	if boldAfter == nil {
		t.Fatal("no bold span when cursor after")
	}
	if boldAfter.State != display.Rendered {
		t.Errorf("bold should be Rendered when cursor exits, got %v", boldAfter.State)
	}

	// Verify: buffer offsets for bold span stay the same regardless of reveal state
	if boldOutside.BufferStart != boldInside.BufferStart {
		t.Errorf("BufferStart changed: outside=%d, inside=%d",
			boldOutside.BufferStart, boldInside.BufferStart)
	}
	if boldOutside.BufferEnd != boldInside.BufferEnd {
		t.Errorf("BufferEnd changed: outside=%d, inside=%d",
			boldOutside.BufferEnd, boldInside.BufferEnd)
	}
}

// ==========================================================================
// Gate 5: Domain package emits semantic spans only, no lipgloss
// ==========================================================================

func TestSyntaxMap_NoLipglossInSpans(t *testing.T) {
	texts := []string{
		"## Heading",
		"**bold** and *italic*",
		"~~strike~~ and `code`",
		"[link](url)",
		"> blockquote",
		"- [ ] task",
		"---",
	}

	sMap := display.NewSyntaxMap()
	for _, text := range texts {
		buf := buffer.New(text)
		_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))

		for lineIdx, line := range snap.Lines {
			for spanIdx, sp := range line.Spans {
				// Lipgloss escape sequences start with \x1b[
				if containsANSI(sp.Text) {
					t.Errorf("line %d span %d contains ANSI/lipgloss escape: %q",
						lineIdx, spanIdx, sp.Text)
				}
			}
		}
	}
}

func containsANSI(s string) bool {
	for i := 0; i < len(s)-1; i++ {
		if s[i] == '\x1b' && s[i+1] == '[' {
			return true
		}
	}
	return false
}

// ==========================================================================
// Additional edge cases
// ==========================================================================

func TestSyntaxMap_CursorAtBoldDelimiterBoundary(t *testing.T) {
	text := "hello **bold** world"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor at exact start of ** (col 6) — inside the span range [6,14)
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(6))
	line := snap.Lines[0]
	for _, sp := range line.Spans {
		if sp.Kind == display.TokenBold {
			if sp.State != display.Revealed {
				t.Errorf("cursor at start of delim should reveal, got %v", sp.State)
			}
		}
	}

	// Cursor at col 13 (last char of right **) — inside [6,14)
	_, snap2 := sMap.Sync(buf, cursor.NewCursorSet(13))
	line2 := snap2.Lines[0]
	for _, sp := range line2.Spans {
		if sp.Kind == display.TokenBold {
			if sp.State != display.Revealed {
				t.Errorf("cursor at last char of right delim should reveal, got %v", sp.State)
			}
		}
	}
}

func TestSyntaxMap_EmptyLine(t *testing.T) {
	text := "hello\n\nworld"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))
	if len(snap.Lines) != 3 {
		t.Fatalf("expected 3 lines, got %d", len(snap.Lines))
	}
	// Empty line should have a single text span
	if len(snap.Lines[1].Spans) != 1 {
		t.Fatalf("expected 1 span on empty line, got %d", len(snap.Lines[1].Spans))
	}
	if snap.Lines[1].Spans[0].Kind != display.TokenText {
		t.Errorf("empty line span kind: got %v, want TokenText", snap.Lines[1].Spans[0].Kind)
	}
}

// ==========================================================================
// Multi-byte UTF-8 coordinate roundtrips inside RENDERED spans
// ==========================================================================

func TestSyntaxMap_CoordinateRoundTrip_MultiByteLink(t *testing.T) {
	text := "hello [café](url) world"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))

	var linkSpan *display.SyntaxSpan
	for i := range snap.Lines[0].Spans {
		if snap.Lines[0].Spans[i].Kind == display.TokenLink {
			linkSpan = &snap.Lines[0].Spans[i]
			break
		}
	}
	if linkSpan == nil {
		t.Fatal("no link span found")
	}
	if linkSpan.State != display.Rendered {
		t.Fatalf("link should be Rendered, got %v", linkSpan.State)
	}
	if linkSpan.CellMap == nil {
		t.Fatal("link CellMap should not be nil")
	}
	if len(linkSpan.CellMap) != 4 {
		t.Errorf("CellMap length: got %d, want 4 (one per rune of 'café')", len(linkSpan.CellMap))
	}

	lineText := buf.Line(0)
	for col := 0; col <= len(lineText); col++ {
		bp := coords.BufferPoint{Line: 0, Col: col}
		sp := snap.BufferToSyntax(bp)
		bp2 := snap.SyntaxToBuffer(sp)

		sp2 := snap.BufferToSyntax(bp2)
		if sp != sp2 {
			t.Errorf("stability violation at col %d: bp=%v → sp=%v → bp2=%v → sp2=%v",
				col, bp, sp, bp2, sp2)
		}
		if bp != bp2 {
			bp3 := snap.SyntaxToBuffer(sp2)
			if bp2 != bp3 {
				t.Errorf("clamped position not stable at col %d: bp2=%v → sp2=%v → bp3=%v",
					col, bp2, sp2, bp3)
			}
		}
	}
}

func TestSyntaxMap_CoordinateRoundTrip_MultiByteBold(t *testing.T) {
	text := "hello **café** world"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))

	var boldSpan *display.SyntaxSpan
	for i := range snap.Lines[0].Spans {
		if snap.Lines[0].Spans[i].Kind == display.TokenBold {
			boldSpan = &snap.Lines[0].Spans[i]
			break
		}
	}
	if boldSpan == nil {
		t.Fatal("no bold span found")
	}
	if boldSpan.State != display.Rendered {
		t.Fatalf("bold should be Rendered, got %v", boldSpan.State)
	}
	if boldSpan.CellMap == nil {
		t.Fatal("bold CellMap should not be nil")
	}
	if len(boldSpan.CellMap) != 4 {
		t.Errorf("CellMap length: got %d, want 4 (one per rune of 'café')", len(boldSpan.CellMap))
	}

	lineText := buf.Line(0)
	for col := 0; col <= len(lineText); col++ {
		bp := coords.BufferPoint{Line: 0, Col: col}
		sp := snap.BufferToSyntax(bp)
		bp2 := snap.SyntaxToBuffer(sp)

		sp2 := snap.BufferToSyntax(bp2)
		if sp != sp2 {
			t.Errorf("stability violation at col %d: bp=%v → sp=%v → bp2=%v → sp2=%v",
				col, bp, sp, bp2, sp2)
		}
		if bp != bp2 {
			bp3 := snap.SyntaxToBuffer(sp2)
			if bp2 != bp3 {
				t.Errorf("clamped position not stable at col %d: bp2=%v → sp2=%v → bp3=%v",
					col, bp2, sp2, bp3)
			}
		}
	}
}

// TestSyntaxMap_InlineSpanAtLineEnd_NoNewline verifies that inline spans at
// the end of a task line do not carry a trailing \n in their Text field.
// This exercises extractChildText's SoftLineBreak handling.
func TestSyntaxMap_InlineSpanAtLineEnd_NoNewline(t *testing.T) {
	cases := []struct {
		name    string
		content string
		kind    display.TokenKind
		want    string
	}{
		{"bold at end", "- [x] **done**\nnext", display.TokenBold, "done"},
		{"italic at end", "- [x] *done*\nnext", display.TokenItalic, "done"},
		{"code at end", "- [x] `done`\nnext", display.TokenInlineCode, "done"},
		{"link at end", "- [x] [done](url)\nnext", display.TokenLink, "done"},
	}
	sMap := display.NewSyntaxMap()
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			buf := buffer.New(tc.content)
			_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))
			for _, sp := range snap.Lines[0].Spans {
				if sp.Kind == tc.kind {
					if strings.Contains(sp.Text, "\n") {
						t.Errorf("span Text contains \\n: %q", sp.Text)
					}
					if sp.Text != tc.want {
						t.Errorf("Text = %q, want %q", sp.Text, tc.want)
					}
				}
			}
		})
	}
}

func TestSyntaxMap_CoordinateRoundTrip_MultiByteCJK(t *testing.T) {
	text := "hello **你好世界** world"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))

	var boldSpan *display.SyntaxSpan
	for i := range snap.Lines[0].Spans {
		if snap.Lines[0].Spans[i].Kind == display.TokenBold {
			boldSpan = &snap.Lines[0].Spans[i]
			break
		}
	}
	if boldSpan == nil {
		t.Fatal("no bold span found")
	}
	if boldSpan.State != display.Rendered {
		t.Fatalf("bold should be Rendered, got %v", boldSpan.State)
	}
	if boldSpan.CellMap == nil {
		t.Fatal("bold CellMap should not be nil")
	}
	if len(boldSpan.CellMap) != 4 {
		t.Errorf("CellMap length: got %d, want 4 (one per CJK char)", len(boldSpan.CellMap))
	}

	lineText := buf.Line(0)
	for col := 0; col <= len(lineText); col++ {
		bp := coords.BufferPoint{Line: 0, Col: col}
		sp := snap.BufferToSyntax(bp)
		bp2 := snap.SyntaxToBuffer(sp)

		sp2 := snap.BufferToSyntax(bp2)
		if sp != sp2 {
			t.Errorf("stability violation at col %d: bp=%v → sp=%v → bp2=%v → sp2=%v",
				col, bp, sp, bp2, sp2)
		}
		if bp != bp2 {
			bp3 := snap.SyntaxToBuffer(sp2)
			if bp2 != bp3 {
				t.Errorf("clamped position not stable at col %d: bp2=%v → sp2=%v → bp3=%v",
					col, bp2, sp2, bp3)
			}
		}
	}
}
