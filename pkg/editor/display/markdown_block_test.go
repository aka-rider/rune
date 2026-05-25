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
// Gate 1: Cursor anywhere in a fence reveals the whole fence
// ==========================================================================

func TestCodeFence_CursorOnOpeningRevealsAll(t *testing.T) {
	text := "before\n```go\nfmt.Println()\n```\nafter"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Place cursor on the opening fence line (line 1: "```go")
	openingOffset := len("before\n")
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(openingOffset))

	// All lines of the block (lines 1, 2, 3) should be Revealed
	for lineIdx := 1; lineIdx <= 3; lineIdx++ {
		line := snap.Lines[lineIdx]
		for _, sp := range line.Spans {
			if sp.Kind == display.TokenCodeFence && sp.State != display.Revealed {
				t.Errorf("line %d: fence span should be Revealed when cursor on opening, got Rendered", lineIdx)
			}
		}
	}
}

func TestCodeFence_CursorOnBodyRevealsAll(t *testing.T) {
	text := "before\n```go\nfmt.Println()\n```\nafter"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Place cursor on a body line (line 2: "fmt.Println()")
	bodyOffset := len("before\n```go\n")
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(bodyOffset))

	// All lines of the block (lines 1, 2, 3) should be Revealed
	for lineIdx := 1; lineIdx <= 3; lineIdx++ {
		line := snap.Lines[lineIdx]
		for _, sp := range line.Spans {
			if sp.Kind == display.TokenCodeFence && sp.State != display.Revealed {
				t.Errorf("line %d: fence span should be Revealed when cursor on body, got Rendered", lineIdx)
			}
		}
	}
}

func TestCodeFence_CursorOnClosingRevealsAll(t *testing.T) {
	text := "before\n```go\nfmt.Println()\n```\nafter"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Place cursor on closing fence line (line 3: "```")
	closingOffset := len("before\n```go\nfmt.Println()\n")
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(closingOffset))

	// All lines of the block (lines 1, 2, 3) should be Revealed
	for lineIdx := 1; lineIdx <= 3; lineIdx++ {
		line := snap.Lines[lineIdx]
		for _, sp := range line.Spans {
			if sp.Kind == display.TokenCodeFence && sp.State != display.Revealed {
				t.Errorf("line %d: fence span should be Revealed when cursor on closing, got Rendered", lineIdx)
			}
		}
	}
}

func TestCodeFence_CursorOutsideRendersBlock(t *testing.T) {
	text := "before\n```go\nfmt.Println()\n```\nafter"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Place cursor outside the block (line 0: "before")
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))

	// Fence marker lines should be Rendered (hidden)
	for _, lineIdx := range []int{1, 3} {
		line := snap.Lines[lineIdx]
		for _, sp := range line.Spans {
			if sp.Kind == display.TokenCodeFence {
				if sp.State != display.Rendered {
					t.Errorf("line %d: fence marker should be Rendered when cursor outside, got Revealed", lineIdx)
				}
				// In rendered mode, fence marker lines have empty text
				if sp.Text != "" {
					t.Errorf("line %d: fence marker rendered text should be empty, got %q", lineIdx, sp.Text)
				}
			}
		}
	}

	// Content line (line 2) should still show text in Rendered state
	contentLine := snap.Lines[2]
	for _, sp := range contentLine.Spans {
		if sp.Kind == display.TokenCodeFence {
			if sp.State != display.Rendered {
				t.Errorf("line 2: code content should be Rendered when cursor outside, got Revealed")
			}
			if sp.Text != "fmt.Println()" {
				t.Errorf("line 2: code content text: got %q, want %q", sp.Text, "fmt.Println()")
			}
		}
	}
}

// ==========================================================================
// Gate 2: Block reveal does not corrupt coordinate round-trips
// ==========================================================================

func TestSyntaxMapBlock_CoordinateRoundTripAtFenceBoundary(t *testing.T) {
	text := "before\n```go\nfmt.Println()\n```\nafter"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Test with cursor outside block (rendered mode with hidden fence markers)
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))

	// For each line in the block, verify round-trip stability
	for lineIdx := 1; lineIdx <= 3; lineIdx++ {
		lineText := buf.Line(lineIdx)
		for col := 0; col <= len(lineText); col++ {
			bp := coords.BufferPoint{Line: lineIdx, Col: col}
			sp := snap.BufferToSyntax(bp)
			bp2 := snap.SyntaxToBuffer(sp)

			// Stability: BufferToSyntax(bp2) must equal sp
			sp2 := snap.BufferToSyntax(bp2)
			if sp != sp2 {
				t.Errorf("stability violated at line %d col %d: bp=%v → sp=%v → bp2=%v → sp2=%v",
					lineIdx, col, bp, sp, bp2, sp2)
			}
		}
	}
}

func TestSyntaxMapBlock_CoordinateRoundTripFirstBlockLine(t *testing.T) {
	text := "before\n```go\nfmt.Println()\n```\nafter"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor outside → fence lines are hidden (rendered)
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))

	// First block line (opening fence "```go")
	bp := coords.BufferPoint{Line: 1, Col: 0}
	sp := snap.BufferToSyntax(bp)
	bp2 := snap.SyntaxToBuffer(sp)
	sp2 := snap.BufferToSyntax(bp2)

	if sp != sp2 {
		t.Errorf("first block line stability: sp=%v, sp2=%v (bp=%v, bp2=%v)", sp, sp2, bp, bp2)
	}
}

func TestSyntaxMapBlock_CoordinateRoundTripLastBlockLine(t *testing.T) {
	text := "before\n```go\nfmt.Println()\n```\nafter"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor outside → fence lines are hidden (rendered)
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))

	// Last block line (closing fence "```")
	bp := coords.BufferPoint{Line: 3, Col: 0}
	sp := snap.BufferToSyntax(bp)
	bp2 := snap.SyntaxToBuffer(sp)
	sp2 := snap.BufferToSyntax(bp2)

	if sp != sp2 {
		t.Errorf("last block line stability: sp=%v, sp2=%v (bp=%v, bp2=%v)", sp, sp2, bp, bp2)
	}
}

func TestSyntaxMapBlock_CoordinateRoundTripRevealed(t *testing.T) {
	text := "before\n```go\nfmt.Println()\n```\nafter"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor inside block → revealed (no hidden delimiters)
	bodyOffset := len("before\n```go\n")
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(bodyOffset))

	// In revealed mode, all positions should round-trip perfectly
	for lineIdx := 1; lineIdx <= 3; lineIdx++ {
		lineText := buf.Line(lineIdx)
		for col := 0; col <= len(lineText); col++ {
			bp := coords.BufferPoint{Line: lineIdx, Col: col}
			sp := snap.BufferToSyntax(bp)
			bp2 := snap.SyntaxToBuffer(sp)

			if bp != bp2 {
				t.Errorf("revealed mode roundtrip failed at line %d col %d: bp=%v → sp=%v → bp2=%v",
					lineIdx, col, bp, sp, bp2)
			}
		}
	}
}

// ==========================================================================
// Gate 3: Table fallback is explicit — tables render as raw text with metadata
// ==========================================================================

func TestMarkdownTable_RenderedVsRevealed(t *testing.T) {
	text := "before\n| A | B |\n|---|---|\n| 1 | 2 |\nafter"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor outside table → rendered
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))

	// Table lines (1, 2, 3) should have TokenTable spans
	for lineIdx := 1; lineIdx <= 3; lineIdx++ {
		line := snap.Lines[lineIdx]
		found := false
		for _, sp := range line.Spans {
			if sp.Kind == display.TokenTable {
				found = true
				if sp.State != display.Rendered {
					t.Errorf("line %d: table span should be Rendered when cursor outside, got Revealed", lineIdx)
				}
				// Table in rendered mode shows raw text (explicit fallback)
				expectedText := buf.Line(lineIdx)
				if sp.Text != expectedText {
					t.Errorf("line %d: table text: got %q, want %q", lineIdx, sp.Text, expectedText)
				}
			}
		}
		if !found {
			t.Errorf("line %d: no table span found", lineIdx)
		}
	}
}

func TestMarkdownTable_CursorInsideReveals(t *testing.T) {
	text := "before\n| A | B |\n|---|---|\n| 1 | 2 |\nafter"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor on table line
	tableOffset := len("before\n")
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(tableOffset))

	// All table lines should be Revealed
	for lineIdx := 1; lineIdx <= 3; lineIdx++ {
		line := snap.Lines[lineIdx]
		for _, sp := range line.Spans {
			if sp.Kind == display.TokenTable {
				if sp.State != display.Revealed {
					t.Errorf("line %d: table span should be Revealed when cursor inside, got Rendered", lineIdx)
				}
			}
		}
	}
}

func TestMarkdownTable_RawTextFallbackExplicit(t *testing.T) {
	// Gate 3: Table fallback is explicit — rendered text is the raw source
	text := "| Col1 | Col2 | Col3 |\n|------|------|------|\n| a    | b    | c    |"
	sMap := display.NewSyntaxMap()

	// Cursor not on table
	_, snap := sMap.Sync(buffer.New("preamble\n"+text), cursor.NewCursorSet(0))

	lines := strings.Split(text, "\n")
	for i, expected := range lines {
		lineIdx := i + 1 // offset by preamble
		found := false
		for _, sp := range snap.Lines[lineIdx].Spans {
			if sp.Kind == display.TokenTable {
				found = true
				if sp.Text != expected {
					t.Errorf("table line %d: rendered text %q != raw source %q", i, sp.Text, expected)
				}
			}
		}
		if !found {
			t.Errorf("table line %d: no TokenTable span", i)
		}
	}
}

// ==========================================================================
// Gate 4: Row-count changes preserve viewport anchor (multi-line fence per-line spans)
// ==========================================================================

func TestCodeFence_MultiLineProducesPerLineSpans(t *testing.T) {
	text := "```python\nline1\nline2\nline3\n```\nafter"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor outside: on "after" line (last line)
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(len("```python\nline1\nline2\nline3\n```\n")))

	// Each buffer line in the block should map to exactly one SyntaxLine
	// (not one giant multi-line span)
	if len(snap.Lines) != buf.LineCount() {
		t.Fatalf("SyntaxLine count: got %d, want %d", len(snap.Lines), buf.LineCount())
	}

	// Check that each line within the block has its own span(s)
	for lineIdx := 0; lineIdx < buf.LineCount()-1; lineIdx++ { // skip "after" line
		sl := snap.Lines[lineIdx]
		if len(sl.Spans) == 0 {
			t.Errorf("line %d: no spans", lineIdx)
			continue
		}
		// Each span's text should correspond to that line's content only
		totalText := ""
		for _, sp := range sl.Spans {
			totalText += sp.Text
		}
		lineText := buf.Line(lineIdx)
		// In rendered mode, fence markers have empty text
		if lineIdx == 0 || lineIdx == 4 {
			if totalText != "" {
				t.Errorf("line %d (fence marker): expected empty rendered text, got %q", lineIdx, totalText)
			}
		} else {
			if totalText != lineText {
				t.Errorf("line %d: span text %q != buffer line %q", lineIdx, totalText, lineText)
			}
		}
	}
}

func TestCodeFence_RevealedMultiLinePerLineSpans(t *testing.T) {
	text := "```python\nline1\nline2\nline3\n```"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor inside: revealed mode
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(len("```python\n")))

	if len(snap.Lines) != buf.LineCount() {
		t.Fatalf("SyntaxLine count: got %d, want %d", len(snap.Lines), buf.LineCount())
	}

	for lineIdx := 0; lineIdx < buf.LineCount(); lineIdx++ {
		sl := snap.Lines[lineIdx]
		if len(sl.Spans) == 0 {
			t.Errorf("line %d: no spans", lineIdx)
			continue
		}
		totalText := ""
		for _, sp := range sl.Spans {
			totalText += sp.Text
		}
		lineText := buf.Line(lineIdx)
		if totalText != lineText {
			t.Errorf("line %d: span text %q != buffer line %q", lineIdx, totalText, lineText)
		}
	}
}

func TestCodeFence_BlockMetadataOnSpans(t *testing.T) {
	text := "```go\nfmt.Println()\n```"
	b := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.Sync(b, cursor.NewCursorSet(len("```go\n")))

	// Check language metadata is propagated
	for lineIdx := 0; lineIdx < b.LineCount(); lineIdx++ {
		for _, sp := range snap.Lines[lineIdx].Spans {
			if sp.Kind == display.TokenCodeFence {
				if sp.Language != "go" {
					t.Errorf("line %d: Language = %q, want %q", lineIdx, sp.Language, "go")
				}
				if sp.BlockID == 0 {
					t.Errorf("line %d: BlockID should be non-zero", lineIdx)
				}
			}
		}
	}
}

func TestCodeFence_EmptyFence(t *testing.T) {
	text := "before\n```\n```\nafter"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor on the empty fence
	fenceOffset := len("before\n")
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(fenceOffset))

	// The opening and closing fence lines should be TokenCodeFence and Revealed
	for _, lineIdx := range []int{1, 2} {
		found := false
		for _, sp := range snap.Lines[lineIdx].Spans {
			if sp.Kind == display.TokenCodeFence {
				found = true
				if sp.State != display.Revealed {
					t.Errorf("line %d: empty fence should be Revealed, got Rendered", lineIdx)
				}
			}
		}
		if !found {
			t.Errorf("line %d: no code fence span found", lineIdx)
		}
	}
}

func TestCodeFence_TildeFence(t *testing.T) {
	text := "~~~\ncode\n~~~"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor inside
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(len("~~~\n")))

	for lineIdx := 0; lineIdx < buf.LineCount(); lineIdx++ {
		found := false
		for _, sp := range snap.Lines[lineIdx].Spans {
			if sp.Kind == display.TokenCodeFence {
				found = true
				if sp.State != display.Revealed {
					t.Errorf("line %d: tilde fence should be Revealed, got Rendered", lineIdx)
				}
			}
		}
		if !found {
			t.Errorf("line %d: no code fence span found", lineIdx)
		}
	}
}

// ==========================================================================
// Non-block lines should not be affected by blocks
// ==========================================================================

func TestCodeFence_NonBlockLinesUnaffected(t *testing.T) {
	text := "before\n```\ncode\n```\nafter"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))

	// Line 0 "before" should be plain text
	line0 := snap.Lines[0]
	if len(line0.Spans) != 1 {
		t.Fatalf("line 0: expected 1 span, got %d", len(line0.Spans))
	}
	if line0.Spans[0].Kind != display.TokenText {
		t.Errorf("line 0: expected TokenText, got %v", line0.Spans[0].Kind)
	}
	if line0.Spans[0].Text != "before" {
		t.Errorf("line 0: text = %q, want %q", line0.Spans[0].Text, "before")
	}

	// Line 4 "after" should be plain text
	line4 := snap.Lines[4]
	if len(line4.Spans) != 1 {
		t.Fatalf("line 4: expected 1 span, got %d", len(line4.Spans))
	}
	if line4.Spans[0].Kind != display.TokenText {
		t.Errorf("line 4: expected TokenText, got %v", line4.Spans[0].Kind)
	}
	if line4.Spans[0].Text != "after" {
		t.Errorf("line 4: text = %q, want %q", line4.Spans[0].Text, "after")
	}
}
