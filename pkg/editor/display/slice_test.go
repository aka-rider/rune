package display

import (
	"testing"
	"unicode/utf8"
)

func ref(index, startOff int) spanRef {
	return spanRef{index: index, startOff: startOff}
}

func TestSliceOriginalSpans_RenderedWithCellMap(t *testing.T) {
	spans := []SyntaxSpan{
		{
			Text:        "asd",
			Kind:        TokenLink,
			State:       Rendered,
			BufferStart: 5,
			BufferEnd:   18,
			CellMap: []CellMapping{
				{BufOffset: 6},
				{BufOffset: 7},
				{BufOffset: 8},
			},
			LinkURL: "https://example.com",
		},
	}
	refs := []spanRef{ref(0, 0)}

	// Slice first 2 bytes: Text="as", CellMap=[{6},{7}]
	result := sliceOriginalSpans(spans, refs, 0, 2)
	if len(result) != 1 {
		t.Fatalf("expected 1 span, got %d", len(result))
	}
	out := result[0]
	if out.Text != "as" {
		t.Errorf("Text: got %q, want %q", out.Text, "as")
	}
	if out.CellMap == nil {
		t.Fatal("CellMap is nil — must be non-nil with correct slice")
	}
	if len(out.CellMap) != 2 {
		t.Fatalf("CellMap length: got %d, want 2", len(out.CellMap))
	}
	if out.CellMap[0].BufOffset != 6 {
		t.Errorf("CellMap[0].BufOffset: got %d, want 6", out.CellMap[0].BufOffset)
	}
	if out.CellMap[1].BufOffset != 7 {
		t.Errorf("CellMap[1].BufOffset: got %d, want 7", out.CellMap[1].BufOffset)
	}
	// Metadata must be preserved
	if out.LinkURL != "https://example.com" {
		t.Errorf("LinkURL: got %q, want %q", out.LinkURL, "https://example.com")
	}
	if out.Kind != TokenLink {
		t.Errorf("Kind: got %v, want TokenLink", out.Kind)
	}
}

func TestSliceOriginalSpans_RenderedFullSpan(t *testing.T) {
	spans := []SyntaxSpan{
		{
			Text:        "asd",
			Kind:        TokenLink,
			State:       Rendered,
			BufferStart: 5,
			BufferEnd:   18,
			CellMap: []CellMapping{
				{BufOffset: 6},
				{BufOffset: 7},
				{BufOffset: 8},
			},
		},
	}
	refs := []spanRef{ref(0, 0)}

	// No-op slice: full span
	result := sliceOriginalSpans(spans, refs, 0, 3)
	if len(result) != 1 {
		t.Fatalf("expected 1 span, got %d", len(result))
	}
	out := result[0]
	if out.Text != "asd" {
		t.Errorf("Text: got %q, want %q", out.Text, "asd")
	}
	if out.CellMap == nil {
		t.Fatal("CellMap is nil — must be preserved for full span")
	}
	if len(out.CellMap) != 3 {
		t.Fatalf("CellMap length: got %d, want 3", len(out.CellMap))
	}
}

func TestSliceOriginalSpans_RenderedMultiByte(t *testing.T) {
	// "café" is 5 bytes, 4 runes — é is 2 bytes
	spans := []SyntaxSpan{
		{
			Text:        "café",
			State:       Rendered,
			BufferStart: 9,
			BufferEnd:   17,
			CellMap: []CellMapping{
				{BufOffset: 10},
				{BufOffset: 11},
				{BufOffset: 12},
				{BufOffset: 13},
			},
		},
	}
	refs := []spanRef{ref(0, 0)}

	// Slice to isolate "é" (bytes 3..5 → rune index 3..4)
	result := sliceOriginalSpans(spans, refs, 3, 5)
	if len(result) != 1 {
		t.Fatalf("expected 1 span, got %d", len(result))
	}
	out := result[0]
	if out.Text != "é" {
		t.Errorf("Text: got %q, want %q", out.Text, "é")
	}
	if out.CellMap == nil {
		t.Fatal("CellMap is nil")
	}
	if len(out.CellMap) != 1 {
		t.Fatalf("CellMap length: got %d, want 1", len(out.CellMap))
	}
	if out.CellMap[0].BufOffset != 13 {
		t.Errorf("CellMap[0].BufOffset: got %d, want 13", out.CellMap[0].BufOffset)
	}
	if !utf8.ValidString(out.Text) {
		t.Errorf("Text is invalid UTF-8: %q", out.Text)
	}
}

func TestSliceOriginalSpans_RenderedCJK(t *testing.T) {
	// "你好世界" is 12 bytes, 4 CJK runes (3 bytes each)
	spans := []SyntaxSpan{
		{
			Text:        "你好世界",
			State:       Rendered,
			BufferStart: 20,
			BufferEnd:   35,
			CellMap: []CellMapping{
				{BufOffset: 20},
				{BufOffset: 23},
				{BufOffset: 26},
				{BufOffset: 29},
			},
		},
	}
	refs := []spanRef{ref(0, 0)}

	// Slice to isolate "世界" (bytes 6..12 → rune index 2..4)
	result := sliceOriginalSpans(spans, refs, 6, 12)
	if len(result) != 1 {
		t.Fatalf("expected 1 span, got %d", len(result))
	}
	out := result[0]
	if out.Text != "世界" {
		t.Errorf("Text: got %q, want %q", out.Text, "世界")
	}
	if out.CellMap == nil {
		t.Fatal("CellMap is nil")
	}
	if len(out.CellMap) != 2 {
		t.Fatalf("CellMap length: got %d, want 2", len(out.CellMap))
	}
	if out.CellMap[0].BufOffset != 26 {
		t.Errorf("CellMap[0].BufOffset: got %d, want 26", out.CellMap[0].BufOffset)
	}
	if out.CellMap[1].BufOffset != 29 {
		t.Errorf("CellMap[1].BufOffset: got %d, want 29", out.CellMap[1].BufOffset)
	}
}

func TestSliceOriginalSpans_RenderedNilCellMap(t *testing.T) {
	spans := []SyntaxSpan{
		{
			Text:        "code",
			Kind:        TokenCodeFence,
			State:       Rendered,
			BufferStart: 0,
			BufferEnd:   4,
			CellMap:     nil,
		},
	}
	refs := []spanRef{ref(0, 0)}

	result := sliceOriginalSpans(spans, refs, 0, 2)
	if len(result) != 1 {
		t.Fatalf("expected 1 span, got %d", len(result))
	}
	out := result[0]
	if out.Text != "co" {
		t.Errorf("Text: got %q, want %q", out.Text, "co")
	}
	if out.CellMap != nil {
		t.Error("CellMap must remain nil for spans that never had one")
	}
}

func TestSliceOriginalSpans_RevealedNilCellMap(t *testing.T) {
	spans := []SyntaxSpan{
		{
			Text:        "hello",
			State:       Revealed,
			BufferStart: 0,
			BufferEnd:   5,
			CellMap:     nil,
		},
	}
	refs := []spanRef{ref(0, 0)}

	result := sliceOriginalSpans(spans, refs, 1, 4)
	if len(result) != 1 {
		t.Fatalf("expected 1 span, got %d", len(result))
	}
	out := result[0]
	if out.Text != "ell" {
		t.Errorf("Text: got %q, want %q", out.Text, "ell")
	}
	if out.CellMap != nil {
		t.Error("Revealed span CellMap must be nil")
	}
	if out.BufferStart != 1 {
		t.Errorf("BufferStart: got %d, want 1", out.BufferStart)
	}
	if out.BufferEnd != 4 {
		t.Errorf("BufferEnd: got %d, want 4", out.BufferEnd)
	}
}
