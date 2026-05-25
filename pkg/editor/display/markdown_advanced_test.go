package display_test

import (
	"testing"

	"rune/pkg/editor/buffer"
	"rune/pkg/editor/coords"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/display"
)

// ==========================================================================
// Gate 1: Advanced elements never perform I/O during SyntaxMap.Sync
// ==========================================================================

func TestMarkdownAdvanced_NoIODuringSync(t *testing.T) {
	// Documents referencing external resources must parse without I/O.
	// If Sync attempted file reads, it would panic or block; this test confirms it doesn't.
	content := "![alt text](/nonexistent/path/image.png)\n" +
		"![[embed-that-does-not-exist]]\n" +
		"$E=mc^2$\n" +
		"$$\nx^2 + y^2 = z^2\n$$\n" +
		"---\ntitle: test\n---\n" +
		"==highlighted==\n" +
		"> [!warning] Danger\n"

	buf := buffer.New(content)
	sMap := display.NewSyntaxMap()
	cursors := cursor.NewCursorSet(0)

	// This must not panic or perform any I/O
	_, snap := sMap.Sync(buf, cursors)

	if len(snap.Lines) == 0 {
		t.Fatal("expected non-empty snapshot")
	}
}

// ==========================================================================
// Gate 2: Image tokens render as semantic placeholders without graphics
// ==========================================================================

func TestImageToken_RenderedShowsAltText(t *testing.T) {
	text := "before ![my image](photo.png) after"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor not on the image span
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))

	line := snap.Lines[0]
	var imgSpan *display.SyntaxSpan
	for i := range line.Spans {
		if line.Spans[i].Kind == display.TokenImage {
			imgSpan = &line.Spans[i]
			break
		}
	}
	if imgSpan == nil {
		t.Fatal("expected TokenImage span")
	}
	if imgSpan.State != display.Rendered {
		t.Errorf("image state: got %v, want Rendered", imgSpan.State)
	}
	if imgSpan.AltText != "my image" {
		t.Errorf("image alt text: got %q, want %q", imgSpan.AltText, "my image")
	}
	if imgSpan.ImagePath != "photo.png" {
		t.Errorf("image path: got %q, want %q", imgSpan.ImagePath, "photo.png")
	}
	// Rendered text should be the alt text (display placeholder)
	if imgSpan.Text != "my image" {
		t.Errorf("image rendered text: got %q, want %q", imgSpan.Text, "my image")
	}
}

func TestImageToken_RevealedShowsRawMarkdown(t *testing.T) {
	text := "![alt](img.jpg)"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor inside the image span
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(3))

	line := snap.Lines[0]
	var imgSpan *display.SyntaxSpan
	for i := range line.Spans {
		if line.Spans[i].Kind == display.TokenImage {
			imgSpan = &line.Spans[i]
			break
		}
	}
	if imgSpan == nil {
		t.Fatal("expected TokenImage span")
	}
	if imgSpan.State != display.Revealed {
		t.Errorf("image state: got %v, want Revealed", imgSpan.State)
	}
	if imgSpan.Text != "![alt](img.jpg)" {
		t.Errorf("revealed image text: got %q, want %q", imgSpan.Text, "![alt](img.jpg)")
	}
}

func TestImageToken_MetadataExtractedWithoutFileRead(t *testing.T) {
	// Even with paths that look like real files, no I/O should occur.
	text := "![screenshot](/usr/local/share/screenshot.png)"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))

	line := snap.Lines[0]
	var imgSpan *display.SyntaxSpan
	for i := range line.Spans {
		if line.Spans[i].Kind == display.TokenImage {
			imgSpan = &line.Spans[i]
			break
		}
	}
	if imgSpan == nil {
		t.Fatal("expected TokenImage span")
	}
	if imgSpan.AltText != "screenshot" {
		t.Errorf("alt text: got %q, want %q", imgSpan.AltText, "screenshot")
	}
	if imgSpan.ImagePath != "/usr/local/share/screenshot.png" {
		t.Errorf("image path: got %q, want %q", imgSpan.ImagePath, "/usr/local/share/screenshot.png")
	}
}

// ==========================================================================
// Gate 3: Frontmatter default is deterministic and configurable
// ==========================================================================

func TestFrontmatter_DefaultIsCollapsed(t *testing.T) {
	sMap := display.NewSyntaxMap()
	if sMap.FrontmatterMode != display.FrontmatterCollapsed {
		t.Errorf("default frontmatter mode: got %v, want FrontmatterCollapsed", sMap.FrontmatterMode)
	}
}

func TestFrontmatter_CollapsedMode(t *testing.T) {
	text := "---\ntitle: Hello\nauthor: World\n---\n# Content"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor on the content line (not inside frontmatter)
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(buf.Len()-1))

	// Line 0 (opening ---) should show collapsed indicator
	line0 := snap.Lines[0]
	found := false
	for _, sp := range line0.Spans {
		if sp.Kind == display.TokenFrontmatter {
			found = true
			if sp.Text != "··· frontmatter ···" {
				t.Errorf("collapsed indicator: got %q, want %q", sp.Text, "··· frontmatter ···")
			}
			if sp.State != display.Rendered {
				t.Errorf("collapsed state: got %v, want Rendered", sp.State)
			}
		}
	}
	if !found {
		t.Error("no frontmatter span on line 0")
	}

	// Lines 1, 2, 3 (body and closing ---) should have empty rendered text
	for lineIdx := 1; lineIdx <= 3; lineIdx++ {
		line := snap.Lines[lineIdx]
		for _, sp := range line.Spans {
			if sp.Kind == display.TokenFrontmatter {
				if sp.Text != "" {
					t.Errorf("line %d: collapsed body text should be empty, got %q", lineIdx, sp.Text)
				}
			}
		}
	}
}

func TestFrontmatter_SourceMode(t *testing.T) {
	text := "---\ntitle: Hello\n---\n# Content"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()
	sMap.FrontmatterMode = display.FrontmatterSource

	_, snap := sMap.Sync(buf, cursor.NewCursorSet(buf.Len()-1))

	// All frontmatter lines should show their source text
	expected := []string{"---", "title: Hello", "---"}
	for i, want := range expected {
		line := snap.Lines[i]
		found := false
		for _, sp := range line.Spans {
			if sp.Kind == display.TokenFrontmatter {
				found = true
				if sp.Text != want {
					t.Errorf("line %d: source mode text: got %q, want %q", i, sp.Text, want)
				}
			}
		}
		if !found {
			t.Errorf("line %d: no frontmatter span found", i)
		}
	}
}

func TestFrontmatter_HiddenMode(t *testing.T) {
	text := "---\ntitle: Hello\n---\n# Content"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()
	sMap.FrontmatterMode = display.FrontmatterHidden

	_, snap := sMap.Sync(buf, cursor.NewCursorSet(buf.Len()-1))

	// All frontmatter lines should render as empty
	for i := 0; i <= 2; i++ {
		line := snap.Lines[i]
		for _, sp := range line.Spans {
			if sp.Kind == display.TokenFrontmatter {
				if sp.Text != "" {
					t.Errorf("line %d: hidden mode text should be empty, got %q", i, sp.Text)
				}
			}
		}
	}
}

func TestFrontmatter_RevealedWhenCursorInside(t *testing.T) {
	text := "---\ntitle: Hello\n---\n# Content"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor on line 1 (inside frontmatter)
	offset := len("---\n") + 2
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(offset))

	// All frontmatter lines should be revealed
	for i := 0; i <= 2; i++ {
		line := snap.Lines[i]
		for _, sp := range line.Spans {
			if sp.Kind == display.TokenFrontmatter && sp.State != display.Revealed {
				t.Errorf("line %d: frontmatter should be Revealed when cursor inside, got Rendered", i)
			}
		}
	}
}

// ==========================================================================
// Gate 4: Highlight/math delimiter deltas preserve round-trips
// ==========================================================================

func TestMarkdownAdvanced_InlineMathDeltas(t *testing.T) {
	text := "before $x^2$ after"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor not on the math span — delimiters hidden
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))

	// Verify rendered text hides the $ delimiters
	line := snap.Lines[0]
	var mathSpan *display.SyntaxSpan
	for i := range line.Spans {
		if line.Spans[i].Kind == display.TokenInlineMath {
			mathSpan = &line.Spans[i]
			break
		}
	}
	if mathSpan == nil {
		t.Fatal("expected TokenInlineMath span")
	}
	if mathSpan.Text != "x^2" {
		t.Errorf("inline math text: got %q, want %q", mathSpan.Text, "x^2")
	}
	if mathSpan.State != display.Rendered {
		t.Errorf("inline math state: got %v, want Rendered", mathSpan.State)
	}

	// Round-trip: buffer→syntax→buffer for position after math
	// "before $x^2$ after"
	// Buffer col of 'a' in "after" = 14
	// Syntax col should be 14 - 2 (two $ removed) = 12
	bp := coords.BufferPoint{Line: 0, Col: 14}
	sp := snap.BufferToSyntax(bp)
	if sp.Col != 12 {
		t.Errorf("BufferToSyntax for col 14: got %d, want 12", sp.Col)
	}
	bpBack := snap.SyntaxToBuffer(sp)
	if bpBack.Col != bp.Col {
		t.Errorf("round-trip failed: got col %d, want %d", bpBack.Col, bp.Col)
	}
}

func TestMarkdownAdvanced_HighlightDeltas(t *testing.T) {
	text := "start ==marked== end"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor not on the highlight span — delimiters hidden
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))

	line := snap.Lines[0]
	var hlSpan *display.SyntaxSpan
	for i := range line.Spans {
		if line.Spans[i].Kind == display.TokenHighlight {
			hlSpan = &line.Spans[i]
			break
		}
	}
	if hlSpan == nil {
		t.Fatal("expected TokenHighlight span")
	}
	if hlSpan.Text != "marked" {
		t.Errorf("highlight text: got %q, want %q", hlSpan.Text, "marked")
	}

	// Highlight: ==marked== has 2 bytes left, 2 bytes right
	// "start ==marked== end"
	//  0123456789...
	// Buffer col of ' ' before "end" = 17
	// After removing == (2) + == (2) = 4 hidden chars: syntax col = 17 - 4 = 13
	bp := coords.BufferPoint{Line: 0, Col: 17}
	sp := snap.BufferToSyntax(bp)
	if sp.Col != 13 {
		t.Errorf("BufferToSyntax for col 17: got %d, want 13", sp.Col)
	}
	bpBack := snap.SyntaxToBuffer(sp)
	if bpBack.Col != bp.Col {
		t.Errorf("round-trip failed: got col %d, want %d", bpBack.Col, bp.Col)
	}
}

func TestMarkdownAdvanced_InlineMathRevealed(t *testing.T) {
	text := "before $x^2$ after"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor inside the math span (col 8, inside "x^2")
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(8))

	line := snap.Lines[0]
	var mathSpan *display.SyntaxSpan
	for i := range line.Spans {
		if line.Spans[i].Kind == display.TokenInlineMath {
			mathSpan = &line.Spans[i]
			break
		}
	}
	if mathSpan == nil {
		t.Fatal("expected TokenInlineMath span")
	}
	if mathSpan.State != display.Revealed {
		t.Errorf("inline math state: got %v, want Revealed", mathSpan.State)
	}
	if mathSpan.Text != "$x^2$" {
		t.Errorf("revealed math text: got %q, want %q", mathSpan.Text, "$x^2$")
	}
}

func TestMarkdownAdvanced_HighlightRevealed(t *testing.T) {
	text := "start ==marked== end"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor inside the highlight span (col 8, inside "marked")
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(8))

	line := snap.Lines[0]
	var hlSpan *display.SyntaxSpan
	for i := range line.Spans {
		if line.Spans[i].Kind == display.TokenHighlight {
			hlSpan = &line.Spans[i]
			break
		}
	}
	if hlSpan == nil {
		t.Fatal("expected TokenHighlight span")
	}
	if hlSpan.State != display.Revealed {
		t.Errorf("highlight state: got %v, want Revealed", hlSpan.State)
	}
	if hlSpan.Text != "==marked==" {
		t.Errorf("revealed highlight text: got %q, want %q", hlSpan.Text, "==marked==")
	}
}

// ==========================================================================
// Block reveal tests: Frontmatter, Callout, Math Block
// ==========================================================================

func TestMarkdownAdvanced_MathBlockRendered(t *testing.T) {
	text := "before\n$$\nx^2 + y^2\n$$\nafter"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor on "after" line (outside the math block)
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(buf.Len()-1))

	// Line 1 ($$) should be rendered as empty
	line1 := snap.Lines[1]
	for _, sp := range line1.Spans {
		if sp.Kind == display.TokenMathBlock {
			if sp.Text != "" {
				t.Errorf("math block delimiter rendered text: got %q, want empty", sp.Text)
			}
		}
	}

	// Line 2 (content) should show math source
	line2 := snap.Lines[2]
	found := false
	for _, sp := range line2.Spans {
		if sp.Kind == display.TokenMathBlock {
			found = true
			if sp.Text != "x^2 + y^2" {
				t.Errorf("math block content: got %q, want %q", sp.Text, "x^2 + y^2")
			}
		}
	}
	if !found {
		t.Error("no math block span on content line")
	}

	// Line 3 (closing $$) should be rendered as empty
	line3 := snap.Lines[3]
	for _, sp := range line3.Spans {
		if sp.Kind == display.TokenMathBlock {
			if sp.Text != "" {
				t.Errorf("closing math delimiter rendered text: got %q, want empty", sp.Text)
			}
		}
	}
}

func TestMarkdownAdvanced_MathBlockRevealed(t *testing.T) {
	text := "before\n$$\nx^2 + y^2\n$$\nafter"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor inside the math block (on the content line)
	offset := len("before\n$$\n") + 2
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(offset))

	// All math block lines (1, 2, 3) should be revealed
	for lineIdx := 1; lineIdx <= 3; lineIdx++ {
		line := snap.Lines[lineIdx]
		for _, sp := range line.Spans {
			if sp.Kind == display.TokenMathBlock && sp.State != display.Revealed {
				t.Errorf("line %d: math block should be Revealed, got Rendered", lineIdx)
			}
		}
	}
}

func TestMarkdownAdvanced_CalloutRendered(t *testing.T) {
	sMap := display.NewSyntaxMap()

	// Cursor off the callout line (use multiline doc)
	multiText := "other\n> [!note] Important info here"
	multiBuf := buffer.New(multiText)
	_, snap := sMap.Sync(multiBuf, cursor.NewCursorSet(0))

	line := snap.Lines[1]
	var calloutSpan *display.SyntaxSpan
	for i := range line.Spans {
		if line.Spans[i].Kind == display.TokenCallout {
			calloutSpan = &line.Spans[i]
			break
		}
	}
	if calloutSpan == nil {
		t.Fatal("expected TokenCallout span")
	}
	if calloutSpan.CalloutKind != "note" {
		t.Errorf("callout kind: got %q, want %q", calloutSpan.CalloutKind, "note")
	}
	if calloutSpan.State != display.Rendered {
		t.Errorf("callout state: got %v, want Rendered", calloutSpan.State)
	}
}

func TestMarkdownAdvanced_CalloutRevealed(t *testing.T) {
	text := "> [!warning] Watch out"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor on the callout line
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(5))

	line := snap.Lines[0]
	var calloutSpan *display.SyntaxSpan
	for i := range line.Spans {
		if line.Spans[i].Kind == display.TokenCallout {
			calloutSpan = &line.Spans[i]
			break
		}
	}
	if calloutSpan == nil {
		t.Fatal("expected TokenCallout span")
	}
	if calloutSpan.State != display.Revealed {
		t.Errorf("callout state: got %v, want Revealed", calloutSpan.State)
	}
	if calloutSpan.CalloutKind != "warning" {
		t.Errorf("callout kind: got %q, want %q", calloutSpan.CalloutKind, "warning")
	}
}

// ==========================================================================
// Embed references
// ==========================================================================

func TestMarkdownAdvanced_EmbedRendered(t *testing.T) {
	text := "see ![[my-note]] here"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor not on the embed span
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))

	line := snap.Lines[0]
	var embedSpan *display.SyntaxSpan
	for i := range line.Spans {
		if line.Spans[i].Kind == display.TokenImage && line.Spans[i].EmbedRef != "" {
			embedSpan = &line.Spans[i]
			break
		}
	}
	if embedSpan == nil {
		t.Fatal("expected embed span (TokenImage with EmbedRef)")
	}
	if embedSpan.EmbedRef != "my-note" {
		t.Errorf("embed ref: got %q, want %q", embedSpan.EmbedRef, "my-note")
	}
	if embedSpan.State != display.Rendered {
		t.Errorf("embed state: got %v, want Rendered", embedSpan.State)
	}
	if embedSpan.Text != "my-note" {
		t.Errorf("embed rendered text: got %q, want %q", embedSpan.Text, "my-note")
	}
}

func TestMarkdownAdvanced_EmbedRevealed(t *testing.T) {
	text := "see ![[my-note]] here"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	// Cursor inside the embed span (col 6 = inside ![[...)
	_, snap := sMap.Sync(buf, cursor.NewCursorSet(6))

	line := snap.Lines[0]
	var embedSpan *display.SyntaxSpan
	for i := range line.Spans {
		if line.Spans[i].Kind == display.TokenImage {
			embedSpan = &line.Spans[i]
			break
		}
	}
	if embedSpan == nil {
		t.Fatal("expected embed span")
	}
	if embedSpan.State != display.Revealed {
		t.Errorf("embed state: got %v, want Revealed", embedSpan.State)
	}
}

// ==========================================================================
// Coordinate delta monotonicity for advanced inline elements
// ==========================================================================

func TestMarkdownAdvanced_DeltaMonotonicity(t *testing.T) {
	// Multiple inline elements on one line: deltas must be monotonically increasing
	text := "a $x$ b ==hi== c"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))

	// Verify monotonicity: for any a < b on the same line, syntaxCol(a) <= syntaxCol(b)
	for col := 0; col < len(text)-1; col++ {
		bpA := coords.BufferPoint{Line: 0, Col: col}
		bpB := coords.BufferPoint{Line: 0, Col: col + 1}
		spA := snap.BufferToSyntax(bpA)
		spB := snap.BufferToSyntax(bpB)
		if spA.Col > spB.Col {
			t.Errorf("monotonicity violated at col %d: syntaxCol(%d)=%d > syntaxCol(%d)=%d",
				col, col, spA.Col, col+1, spB.Col)
		}
	}
}

func TestMarkdownAdvanced_MultiElementRoundTrip(t *testing.T) {
	text := "a $x$ b ==hi== c"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))

	// Round-trip every cursor-legal position
	for col := 0; col <= len(text); col++ {
		bp := coords.BufferPoint{Line: 0, Col: col}
		sp := snap.BufferToSyntax(bp)
		bpBack := snap.SyntaxToBuffer(sp)
		// The round-trip should land at a legal buffer position
		// (may differ from original if original was in a hidden range)
		if bpBack.Line != 0 {
			t.Errorf("col %d: round-trip changed line", col)
		}
		// The round-trip from legal positions should be stable
		sp2 := snap.BufferToSyntax(bpBack)
		if sp2.Col != sp.Col {
			t.Errorf("col %d: double-convert not stable: %d != %d", col, sp2.Col, sp.Col)
		}
	}
}

// ==========================================================================
// Frontmatter: no frontmatter detection when not at line 0
// ==========================================================================

func TestFrontmatter_NotDetectedMidDocument(t *testing.T) {
	// --- not at line 0 should not be treated as frontmatter
	text := "# Title\n---\nauthor: test\n---\nContent"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))

	// Line 1 should NOT be TokenFrontmatter (it's a horizontal rule or text)
	line1 := snap.Lines[1]
	for _, sp := range line1.Spans {
		if sp.Kind == display.TokenFrontmatter {
			t.Error("--- at line 1 should not be detected as frontmatter")
		}
	}
}

// ==========================================================================
// Inline math edge cases
// ==========================================================================

func TestMarkdownAdvanced_InlineMathEscapedDollar(t *testing.T) {
	// Escaped dollar should not open inline math
	text := `price is \$5 and $x$ is var`
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))

	line := snap.Lines[0]
	mathCount := 0
	for _, sp := range line.Spans {
		if sp.Kind == display.TokenInlineMath {
			mathCount++
			if sp.Text != "x" {
				t.Errorf("inline math text: got %q, want %q", sp.Text, "x")
			}
		}
	}
	if mathCount != 1 {
		t.Errorf("expected 1 math span, got %d", mathCount)
	}
}

func TestMarkdownAdvanced_DoubleDollarNotInlineMath(t *testing.T) {
	// $$ should not be treated as inline math
	text := "some $$block$$ delimiter"
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))

	line := snap.Lines[0]
	for _, sp := range line.Spans {
		if sp.Kind == display.TokenInlineMath {
			t.Errorf("$$ should not produce inline math, got span: %q", sp.Text)
		}
	}
}

// ==========================================================================
// Highlight edge cases
// ==========================================================================

func TestMarkdownAdvanced_HighlightMultiple(t *testing.T) {
	text := "==one== normal ==two=="
	buf := buffer.New(text)
	sMap := display.NewSyntaxMap()

	_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))

	line := snap.Lines[0]
	hlCount := 0
	for _, sp := range line.Spans {
		if sp.Kind == display.TokenHighlight {
			hlCount++
		}
	}
	if hlCount != 2 {
		t.Errorf("expected 2 highlight spans, got %d", hlCount)
	}
}

// ==========================================================================
// Callout kind extraction
// ==========================================================================

func TestMarkdownAdvanced_CalloutKinds(t *testing.T) {
	tests := []struct {
		line string
		kind string
	}{
		{"> [!note] A note", "note"},
		{"> [!warning] Be careful", "warning"},
		{"> [!tip] Pro tip", "tip"},
		{"> [!info] FYI", "info"},
	}

	for _, tt := range tests {
		text := "other line\n" + tt.line
		buf := buffer.New(text)
		sMap := display.NewSyntaxMap()

		_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))

		line := snap.Lines[1]
		var calloutSpan *display.SyntaxSpan
		for i := range line.Spans {
			if line.Spans[i].Kind == display.TokenCallout {
				calloutSpan = &line.Spans[i]
				break
			}
		}
		if calloutSpan == nil {
			t.Errorf("line %q: no callout span found", tt.line)
			continue
		}
		if calloutSpan.CalloutKind != tt.kind {
			t.Errorf("line %q: callout kind got %q, want %q", tt.line, calloutSpan.CalloutKind, tt.kind)
		}
	}
}
