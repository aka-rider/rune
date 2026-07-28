package display_test

import (
	"testing"

	"rune/pkg/editor/buffer"
	"rune/pkg/editor/coords"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/display"
)

// TestWrapSnapshot_OutOfRangeRowClampsToLastRow locks in the fix for the
// cursor-down wraparound bug: WrapSnapshot's row-indexed accessors must clamp
// an out-of-range row to the nearest valid row instead of silently returning
// a zero-value result. Before the fix, WrapToSyntax(row >= TotalRows) reset to
// {Line: 0, Col: 0} — indistinguishable from a legitimate document-start
// position — which is exactly what let an upstream display-space/wrap-space
// row-count mismatch teleport the cursor to line 0 instead of holding it at
// the last line.
func TestWrapSnapshot_OutOfRangeRowClampsToLastRow(t *testing.T) {
	content := "one\ntwo\nthree"
	buf := buffer.New(content)
	cursors := cursor.NewCursorSet(0)
	sMap := display.NewSyntaxMap()
	_, sSnap := sMap.Sync(buf, cursors)
	wMap := display.NewWrapMap(80)
	wSnap := wMap.Sync(sSnap)

	lastRow := wSnap.TotalRows - 1
	if lastRow < 0 {
		t.Fatalf("expected at least one wrap row")
	}
	wantLine := wSnap.RowToModelLine(lastRow)
	wantLen := wSnap.SegmentLen(lastRow)

	outOfRange := wSnap.TotalRows + 5

	if sp := wSnap.WrapToSyntax(coords.WrapPoint{Row: outOfRange, Col: 3}); sp.Line != wantLine {
		t.Fatalf("WrapToSyntax(out-of-range row) = line %d, want %d (last line, not 0)", sp.Line, wantLine)
	}
	if got := wSnap.RowToModelLine(outOfRange); got != wantLine {
		t.Fatalf("RowToModelLine(out-of-range) = %d, want %d", got, wantLine)
	}
	if got := wSnap.SegmentLen(outOfRange); got != wantLen {
		t.Fatalf("SegmentLen(out-of-range) = %d, want %d", got, wantLen)
	}

	// A negative row clamps to the first row rather than degrading to a
	// zero-value that happens to look like row 0 here — assert it resolves
	// identically to explicitly asking for row 0.
	wantFirstLine := wSnap.RowToModelLine(0)
	if got := wSnap.RowToModelLine(-1); got != wantFirstLine {
		t.Fatalf("RowToModelLine(-1) = %d, want %d", got, wantFirstLine)
	}
}
