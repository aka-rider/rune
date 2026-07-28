package buffer

import (
	"testing"
	"unicode/utf8"

	"rune/pkg/editor/coords"
)

func TestBuffer_FromBytes(t *testing.T) {
	// Valid UTF-8
	b, err := FromBytes([]byte("Hello \xe2\x98\xba World"))
	if err != nil {
		t.Errorf("expected no error for valid utf-8, got %v", err)
	}
	if b.Content() != "Hello \xe2\x98\xba World" {
		t.Errorf("content mismatch")
	}

	// Invalid UTF-8
	_, err = FromBytes([]byte{0xff, 0xfe})
	if err == nil {
		t.Errorf("expected error for invalid utf-8")
	}
}

func TestBuffer_ApplyEdits_DescendingOrderAndOverlap(t *testing.T) {
	b := New("hello world")

	// Ascending order (should fail)
	_, _, err := b.ApplyEdits([]Edit{
		{Start: 0, End: 5, Insert: "a"},
		{Start: 6, End: 11, Insert: "b"},
	})
	if err == nil {
		t.Errorf("expected error for ascending edits")
	}

	// Overlapping (should fail)
	_, _, err = b.ApplyEdits([]Edit{
		{Start: 5, End: 10, Insert: "a"},
		{Start: 0, End: 6, Insert: "b"},
	})
	if err == nil {
		t.Errorf("expected error for overlapping edits")
	}

	// Correct (should pass)
	_, _, err = b.ApplyEdits([]Edit{
		{Start: 6, End: 11, Insert: "b"},
		{Start: 0, End: 5, Insert: "a"},
	})
	if err != nil {
		t.Errorf("expected no error, got %v", err)
	}
}

func TestBuffer_CloneAndSortEditsDescending(t *testing.T) {
	edits := []Edit{
		{Start: 0, End: 5, Insert: "a"},
		{Start: 6, End: 11, Insert: "b"},
	}
	sorted := CloneAndSortEditsDescending(edits)

	if edits[0].Start == 6 {
		t.Errorf("original slice was mutated")
	}

	if sorted[0].Start != 6 || sorted[1].Start != 0 {
		t.Errorf("slice not sorted descending correctly")
	}
}

func TestBuffer_LineIndex(t *testing.T) {
	b := New("line 1\nline 2\nline 3")
	if b.LineCount() != 3 {
		t.Errorf("expected 3 lines, got %d", b.LineCount())
	}

	if b.Line(0) != "line 1" {
		t.Errorf("expected 'line 1', got '%s'", b.Line(0))
	}

	bp := b.OffsetToLineCol(10) // "line 1\nlin|e 2"
	if bp.Line != 1 || bp.Col != 3 {
		t.Errorf("expected 1,3 got %d,%d", bp.Line, bp.Col)
	}

	offset := b.LineColToOffset(bp)
	if offset != 10 {
		t.Errorf("expected 10, got %d", offset)
	}
}

func FuzzBufferSnapshotImmutability(f *testing.F) {
	f.Add("hello world", 0, 5, "goodbye")
	f.Fuzz(func(t *testing.T, init string, start int, end int, insert string) {
		if !utf8.ValidString(init) || !utf8.ValidString(insert) {
			return
		}

		b := New(init)
		if start < 0 {
			start = 0
		}
		if start > len(init) {
			start = len(init)
		}
		if end < start {
			end = start
		}
		if end > len(init) {
			end = len(init)
		}

		origLen := b.Len()
		origContent := b.Content()

		newB := b.Replace(start, end, insert)

		if b.Len() != origLen || b.Content() != origContent {
			t.Errorf("buffer mutated! orig %q became %q", origContent, b.Content())
		}

		if newB.Len() == 0 && b.Len() != 0 && origLen != 0 {
			// Just accessing something
		}

		if newB.Len() != len(newB.Content()) {
			t.Errorf("Len mismatch: %d vs actual %d", newB.Len(), len(newB.Content()))
		}

		if utf8.ValidString(init) && utf8.ValidString(insert) {
			if !utf8.ValidString(newB.Content()) {
				t.Errorf("produced invalid utf8")
			}
		}
	})
}

func FuzzBufferBatchEquivalence(f *testing.F) {
	f.Add("hello world", 0, 5, "A", 6, 11, "B")
	f.Fuzz(func(t *testing.T, init string, s1, e1 int, i1 string, s2, e2 int, i2 string) {
		if !utf8.ValidString(init) || !utf8.ValidString(i1) || !utf8.ValidString(i2) {
			return
		}

		// Force bounds
		s1, e1 = normalizeBounds(len(init), s1, e1)
		s2, e2 = normalizeBounds(len(init), s2, e2)

		// Ensure non-overlapping, s1 is after s2
		if s1 < e2 {
			return
		}

		b := New(init)

		// Apply individually
		bIndiv := b.Replace(s1, e1, i1)
		// calculate new bounds for s2, e2 since text before it hasn't changed because s1 >= e2
		bIndiv = bIndiv.Replace(s2, e2, i2)

		bBatch, _, err := b.ApplyEdits([]Edit{
			{Start: s1, End: e1, Insert: i1},
			{Start: s2, End: e2, Insert: i2},
		})
		if err != nil {
			return
		}

		if bIndiv.Content() != bBatch.Content() {
			t.Errorf("batch mismatch: indiv %q vs batch %q", bIndiv.Content(), bBatch.Content())
		}
	})
}

func normalizeBounds(length, start, end int) (int, int) {
	if start < 0 {
		start = 0
	}
	if start > length {
		start = length
	}
	if end < start {
		end = start
	}
	if end > length {
		end = length
	}
	return start, end
}

func FuzzBufferPointRoundtrip(f *testing.F) {
	f.Add("hello\nworld\n", 1, 2)
	f.Fuzz(func(t *testing.T, init string, line, col int) {
		if !utf8.ValidString(init) {
			return
		}
		b := New(init)
		if line < 0 || line >= b.LineCount() {
			return
		}

		start := b.LineStart(line)
		end := b.LineEnd(line)
		if line == len(b.getLineStarts())-1 {
			end = len(b.content) // the end itself
		}
		if col < 0 || col > end-start {
			return
		}

		bp := coords.BufferPoint{Line: line, Col: col}
		offset := b.LineColToOffset(bp)
		bp2 := b.OffsetToLineCol(offset)
		if bp != bp2 {
			t.Errorf("roundtrip failed: %v -> %d -> %v (len %d)", bp, offset, bp2, len(b.content))
		}
	})
}
