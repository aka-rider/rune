package display_test

import (
	"testing"

	"rune/pkg/editor/buffer"
	"rune/pkg/editor/coords"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/display"
)

// Gate 1: P5a SyntaxMap coordinate round-trip invariant
// After WP14A, the map is no longer pass-through for markdown content.
// The invariant is: for cursor-legal positions, SyntaxToBuffer(BufferToSyntax(bp)) == bp.
// For non-cursor-legal positions (inside hidden delimiters), the clamped position must be stable:
// BufferToSyntax(SyntaxToBuffer(BufferToSyntax(bp))) == BufferToSyntax(bp)
func FuzzSyntaxMapRoundtrip(f *testing.F) {
	f.Add("hello world\nline 2\nanother line indeed", 0)
	f.Add("hello", 2)
	f.Add("h\ni\n", 0)
	f.Add("", 0)

	f.Fuzz(func(t *testing.T, text string, offset int) {
		buf := buffer.New(text)
		if offset < 0 {
			offset = 0
		}
		if offset > buf.Len() {
			offset = buf.Len()
		}

		cursors := cursor.NewCursorSet(offset)
		sMap := display.NewSyntaxMap()
		_, sSnapshot := sMap.Sync(buf, cursors)

		for line := 0; line < buf.LineCount(); line++ {
			lineText := buf.Line(line)
			for col := 0; col <= len(lineText); col++ {
				bp := coords.BufferPoint{Line: line, Col: col}
				sp := sSnapshot.BufferToSyntax(bp)
				bp2 := sSnapshot.SyntaxToBuffer(sp)

				// The clamped position must be stable (idempotent)
				sp2 := sSnapshot.BufferToSyntax(bp2)
				if sp != sp2 {
					t.Errorf("Stability violated: bp=%v → sp=%v → bp2=%v → sp2=%v", bp, sp, bp2, sp2)
				}

				// If bp roundtripped, it's cursor-legal
				// If not, verify bp2 itself is stable (cursor-legal)
				if bp != bp2 {
					bp3 := sSnapshot.SyntaxToBuffer(sp2)
					if bp2 != bp3 {
						t.Errorf("Cursor-legal roundtrip failed: bp2=%v → sp2=%v → bp3=%v", bp2, sp2, bp3)
					}
				}
			}
		}
	})
}

// Gate 2: P5b WrapToSyntax(SyntaxToWrap(sp)) == sp
func FuzzWrapMapRoundtrip(f *testing.F) {
	f.Add("hello world\nline 2\nanother line indeed", 10)
	f.Add("hello", 2)
	f.Add("word1 word2 word3", 5)

	f.Fuzz(func(t *testing.T, text string, width int) {
		if width < 1 {
			width = 10
		}
		buf := buffer.New(text)
		cursors := cursor.NewCursorSet(0)
		sMap := display.NewSyntaxMap()
		_, sSnapshot := sMap.Sync(buf, cursors)

		wMap := display.NewWrapMap(width)
		wSnapshot := wMap.Sync(sSnapshot)

		for line := 0; line < buf.LineCount(); line++ {
			lineText := buf.Line(line)
			for col := 0; col <= len(lineText); col++ {
				sp := coords.SyntaxPoint{Line: line, Col: col}
				wp := wSnapshot.SyntaxToWrap(sp)
				sp2 := wSnapshot.WrapToSyntax(wp)
				if sp != sp2 {
					t.Errorf("SyntaxToWrap/WrapToSyntax roundtrip failed: sp=%v, wp=%v, sp2=%v (width=%d)", sp, wp, sp2, width)
				}
			}
		}
	})
}
