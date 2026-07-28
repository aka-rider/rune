package display_test

import (
	"testing"
	"unicode/utf8"

	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/display"
)

// TestWrapMap_MultiSpanBufferOffsets verifies that wrap segments produce correct
// BufferStart/BufferEnd when a line has multiple syntax spans (e.g., plain text
// + rendered inline code + plain text). This is the regression test for the bug
// where the wrap map used only Spans[0].BufferStart as the base, producing
// incorrect buffer offsets for segments spanning multiple original spans.
func TestWrapMap_MultiSpanBufferOffsets(t *testing.T) {
	// Content: "hello `code` world end"
	// Line 0 with cursor at offset 0 (on this line = revealed inline code)
	// Then move cursor OFF line (to a different line) to test rendered mode.
	content := "hello `code` world end\nsecond line"
	buf := buffer.New(content)

	// Cursor on line 1 (NOT line 0) so inline code on line 0 is Rendered.
	cursors := cursor.NewCursorSet(buf.LineStart(1))
	sMap := display.NewSyntaxMap()
	_, sSnap := sMap.Sync(buf, cursors)

	// In rendered mode, line 0 should have spans where inline code backticks
	// are hidden. The rendered text of line 0 will be shorter than buffer text.
	line0 := sSnap.Lines[0]
	if len(line0.Spans) < 2 {
		t.Fatalf("expected multiple spans on line 0 (rendered inline code), got %d", len(line0.Spans))
	}

	// Verify that the total text length of all spans != buffer line length
	// (because delimiters are hidden in rendered mode).
	totalTextLen := 0
	for _, sp := range line0.Spans {
		totalTextLen += len(sp.Text)
	}
	bufLineLen := len(buf.Line(0))
	if totalTextLen >= bufLineLen {
		t.Logf("spans: %+v", line0.Spans)
		t.Fatalf("expected rendered text (%d) to be shorter than buffer line (%d) due to hidden backticks",
			totalTextLen, bufLineLen)
	}

	// Wrap at a width that forces a break somewhere in the line.
	// "hello code world end" is 20 visible chars; wrap at 12 to force split.
	wMap := display.NewWrapMap(12)
	wSnap := wMap.Sync(sSnap)

	// Verify all wrap segments for line 0 have valid BufferStart/BufferEnd.
	for i, seg := range wSnap.Segments {
		if seg.ModelLine != 0 {
			continue
		}
		for j, sp := range seg.Spans {
			// BufferStart must be >= start of line 0 in the buffer.
			if sp.BufferStart < 0 {
				t.Errorf("segment %d span %d: negative BufferStart %d", i, j, sp.BufferStart)
			}
			// BufferEnd must be > BufferStart (non-empty spans).
			if len(sp.Text) > 0 && sp.BufferEnd <= sp.BufferStart {
				t.Errorf("segment %d span %d: BufferEnd (%d) <= BufferStart (%d) for text %q",
					i, j, sp.BufferEnd, sp.BufferStart, sp.Text)
			}
			// BufferEnd must not exceed the end of line 0 in the buffer.
			lineEnd := buf.LineStart(0) + bufLineLen
			if sp.BufferEnd > lineEnd {
				t.Errorf("segment %d span %d: BufferEnd (%d) exceeds line end (%d) for text %q",
					i, j, sp.BufferEnd, lineEnd, sp.Text)
			}
			// For Revealed spans: BufferEnd - BufferStart must equal len(Text).
			if sp.State == display.Revealed && sp.BufferEnd-sp.BufferStart != len(sp.Text) {
				t.Errorf("segment %d span %d: Revealed span BufferEnd-BufferStart (%d) != len(Text) (%d), text=%q",
					i, j, sp.BufferEnd-sp.BufferStart, len(sp.Text), sp.Text)
			}
			// For Rendered spans: buffer range must be >= text length (includes hidden bytes).
			if sp.State == display.Rendered && sp.BufferEnd-sp.BufferStart < len(sp.Text) {
				t.Errorf("segment %d span %d: Rendered span buffer range (%d) < text length (%d), text=%q",
					i, j, sp.BufferEnd-sp.BufferStart, len(sp.Text), sp.Text)
			}
		}
	}
}

// TestWrapMap_HorizontalRuleBufferOffsets verifies that the horizontal rule
// ("---") rendered as "───" produces correct BufferStart/BufferEnd in wrap
// segments and does not corrupt adjacent line offsets.
func TestWrapMap_HorizontalRuleBufferOffsets(t *testing.T) {
	content := "paragraph text here\n\n---\n\n### Heading"
	buf := buffer.New(content)

	// Cursor on the empty line AFTER the HR (line 3) — HR in rendered mode.
	cursors := cursor.NewCursorSet(buf.LineStart(3))
	sMap := display.NewSyntaxMap()
	_, sSnap := sMap.Sync(buf, cursors)

	wMap := display.NewWrapMap(80)
	wSnap := wMap.Sync(sSnap)

	// Find the segment for line 2 (the HR line).
	hrLineStart := buf.LineStart(2)
	hrLineEnd := hrLineStart + len(buf.Line(2))

	for i, seg := range wSnap.Segments {
		if seg.ModelLine != 2 {
			continue
		}
		for j, sp := range seg.Spans {
			// The HR span's buffer range must cover exactly the "---" text.
			if sp.BufferStart < hrLineStart {
				t.Errorf("HR segment %d span %d: BufferStart (%d) < line start (%d)",
					i, j, sp.BufferStart, hrLineStart)
			}
			if sp.BufferEnd > hrLineEnd {
				t.Errorf("HR segment %d span %d: BufferEnd (%d) > line end (%d)",
					i, j, sp.BufferEnd, hrLineEnd)
			}
			// The rendered text "───" must be valid UTF-8.
			if !utf8.ValidString(sp.Text) {
				t.Errorf("HR segment %d span %d: invalid UTF-8 in text: %q", i, j, sp.Text)
			}
		}
	}

	// Verify the heading line (line 4) has correct buffer offsets too.
	headingLineStart := buf.LineStart(4)
	headingLineEnd := headingLineStart + len(buf.Line(4))

	for i, seg := range wSnap.Segments {
		if seg.ModelLine != 4 {
			continue
		}
		for j, sp := range seg.Spans {
			if sp.BufferStart < headingLineStart {
				t.Errorf("heading segment %d span %d: BufferStart (%d) < line start (%d)",
					i, j, sp.BufferStart, headingLineStart)
			}
			if sp.BufferEnd > headingLineEnd {
				t.Errorf("heading segment %d span %d: BufferEnd (%d) > line end (%d)",
					i, j, sp.BufferEnd, headingLineEnd)
			}
		}
	}
}

// TestWrapMap_MultiSpanNoMidRuneSplit verifies that wrap breaks within
// multi-byte characters don't produce invalid UTF-8 in span text.
func TestWrapMap_MultiSpanNoMidRuneSplit(t *testing.T) {
	// HR rendered as "───" (9 bytes, 3 cells) — wrap at 2 cells to force mid-rune potential.
	content := "ab\n\n---\n\ncd"
	buf := buffer.New(content)

	// Cursor on line 0 so HR (line 2) is rendered as "───".
	cursors := cursor.NewCursorSet(0)
	sMap := display.NewSyntaxMap()
	_, sSnap := sMap.Sync(buf, cursors)

	wMap := display.NewWrapMap(2) // Very narrow — forces breaks
	wSnap := wMap.Sync(sSnap)

	for i, seg := range wSnap.Segments {
		for j, sp := range seg.Spans {
			if !utf8.ValidString(sp.Text) {
				t.Errorf("segment %d span %d (line %d): invalid UTF-8 text: %q",
					i, j, seg.ModelLine, sp.Text)
			}
		}
	}
}

// TestWrapMap_WikiLinkImageMetadataPreserved verifies that WikiLinkIsImage,
// WikiLinkTarget, and ImagePath survive wrapping through sliceOriginalSpans.
// This is the regression test for the metadata loss that turned wiki image
// embeds into plain blue links after SetSize.
func TestWrapMap_WikiLinkImageMetadataPreserved(t *testing.T) {
	// A wiki image embed that's long enough to wrap at width 30.
	content := "![[Do not try to DRY.webp]]\nsecond line"
	buf := buffer.New(content)

	// Cursor on line 1 so the wiki embed on line 0 stays Rendered.
	cursors := cursor.NewCursorSet(buf.LineStart(1))
	sMap := display.NewSyntaxMap()
	_, sSnap := sMap.Sync(buf, cursors)

	// Verify the syntax snap has the wiki image span before wrapping.
	line0 := sSnap.Lines[0]
	foundWiki := false
	for _, sp := range line0.Spans {
		if sp.Kind == display.TokenWikiLink && sp.WikiLinkIsImage {
			foundWiki = true
			break
		}
	}
	if !foundWiki {
		t.Fatalf("expected TokenWikiLink with WikiLinkIsImage=true in syntax snap, got: %+v", line0.Spans)
	}

	// Wrap at 30 — shorter than "Do not try to DRY.webp" rendered text would
	// be if fully revealed, but for Rendered span the display text is shorter.
	// Use width 15 to ensure wrapping if the span is wide enough.
	wMap := display.NewWrapMap(15)
	wSnap := wMap.Sync(sSnap)

	// Check that at least one segment span for line 0 retains wiki metadata.
	foundAfterWrap := false
	for _, seg := range wSnap.Segments {
		if seg.ModelLine != 0 {
			continue
		}
		for _, sp := range seg.Spans {
			if sp.Kind == display.TokenWikiLink && sp.WikiLinkIsImage {
				foundAfterWrap = true
				if sp.ImagePath == "" {
					t.Error("WikiLinkIsImage span has empty ImagePath after wrapping")
				}
			}
		}
	}
	if !foundAfterWrap {
		t.Error("WikiLinkIsImage metadata lost after WrapMap.Sync — sliceOriginalSpans drops wiki fields")
	}
}

// TestWrapMap_WrappedLinkCellMap verifies that when a Rendered link span wraps
// across multiple display lines, each segment retains a correctly-sliced CellMap.
func TestWrapMap_WrappedLinkCellMap(t *testing.T) {
	content := "text with [very-long-link-text-that-must-wrap](url) suffix\ncursor line"
	buf := buffer.New(content)

	// Cursor on line 1 (not line 0) → line 0 link is Rendered.
	cursors := cursor.NewCursorSet(buf.LineStart(1))
	sMap := display.NewSyntaxMap()
	_, sSnap := sMap.Sync(buf, cursors)

	// Wrap at a width that forces a break inside the link text.
	// Concatenated visible text: "text with very-long-link-text-that-must-wrap suffix" = 51 bytes.
	// Width 20 forces at least one break within the link text.
	wMap := display.NewWrapMap(20)
	wSnap := wMap.Sync(sSnap)

	for i, seg := range wSnap.Segments {
		if seg.ModelLine != 0 {
			continue
		}
		for j, sp := range seg.Spans {
			if sp.State != display.Rendered {
				continue
			}
			if len(sp.Text) == 0 {
				continue
			}
			if sp.CellMap == nil {
				t.Errorf("segment %d span %d (line 0): Rendered span text=%q has nil CellMap",
					i, j, sp.Text)
				continue
			}
			wantLen := utf8.RuneCountInString(sp.Text)
			if len(sp.CellMap) != wantLen {
				t.Errorf("segment %d span %d (line 0): CellMap length %d != Text rune count %d, text=%q",
					i, j, len(sp.CellMap), wantLen, sp.Text)
			}
			for k, cm := range sp.CellMap {
				if cm.BufOffset < 0 {
					t.Errorf("segment %d span %d CellMap[%d]: negative BufOffset=%d",
						i, j, k, cm.BufOffset)
				}
			}
		}
	}
}

func TestWrapMap_WrappedLinkMetadataPreserved(t *testing.T) {
	content := "text with [very-long-link-text-that-must-wrap](https://example.com) suffix\ncursor line"
	buf := buffer.New(content)

	cursors := cursor.NewCursorSet(buf.LineStart(1))
	sMap := display.NewSyntaxMap()
	_, sSnap := sMap.Sync(buf, cursors)

	wMap := display.NewWrapMap(20)
	wSnap := wMap.Sync(sSnap)

	found := false
	for _, seg := range wSnap.Segments {
		if seg.ModelLine != 0 {
			continue
		}
		for _, sp := range seg.Spans {
			if sp.Kind == display.TokenLink {
				found = true
				if sp.LinkURL != "https://example.com" {
					t.Errorf("wrapped link segment lost LinkURL: got %q, want %q",
						sp.LinkURL, "https://example.com")
				}
			}
		}
	}
	if !found {
		t.Error("no TokenLink span found in wrapped output")
	}
}

func TestWrapMap_ShortLinkPassesThrough(t *testing.T) {
	content := "text [link](url) more\ncursor line"
	buf := buffer.New(content)

	cursors := cursor.NewCursorSet(buf.LineStart(1))
	sMap := display.NewSyntaxMap()
	_, sSnap := sMap.Sync(buf, cursors)

	// Width 80 is wide enough — the short link should not be split.
	wMap := display.NewWrapMap(80)
	wSnap := wMap.Sync(sSnap)

	for _, seg := range wSnap.Segments {
		if seg.ModelLine != 0 {
			continue
		}
		for _, sp := range seg.Spans {
			if sp.Kind == display.TokenLink && sp.State == display.Rendered {
				if sp.CellMap == nil {
					t.Error("non-wrapped rendered link has nil CellMap")
				}
				if sp.Text != "link" {
					t.Errorf("non-wrapped rendered link text: got %q, want %q", sp.Text, "link")
				}
				if len(sp.CellMap) != 4 {
					t.Errorf("non-wrapped rendered link CellMap length: got %d, want 4",
						len(sp.CellMap))
				}
				// Verify first cell maps past the left delimiter '['
				if sp.CellMap[0].BufOffset < sp.BufferStart {
					t.Errorf("CellMap[0].BufOffset %d < BufferStart %d — must point past left delim",
						sp.CellMap[0].BufOffset, sp.BufferStart)
				}
			}
		}
	}
}
