package display

import (
	"sort"

	"rune/pkg/editor/coords"
)

type OffsetDelta struct {
	BufferOffset int
	Delta        int
}

// hiddenRange represents a non-cursor-legal buffer column range.
type hiddenRange struct {
	start   int // first non-legal col (inclusive)
	end     int // first legal col after (exclusive)
	clampTo int // buffer col to clamp to
}

// lineConversion holds per-line data for coordinate conversion.
type lineConversion struct {
	deltas []OffsetDelta // sorted by BufferOffset, monotonically increasing Delta
	hidden []hiddenRange // sorted by start
}

type SyntaxSnapshot struct {
	Lines     []SyntaxLine
	Deltas    []OffsetDelta // global monotonic (for external consumers)
	lineConvs []lineConversion
}

// BufferToSyntax converts a buffer-space point to syntax-space.
// Positions inside hidden delimiters are clamped to cursor-legal positions.
func (s SyntaxSnapshot) BufferToSyntax(bp coords.BufferPoint) coords.SyntaxPoint {
	if bp.Line < 0 || bp.Line >= len(s.lineConvs) {
		return coords.SyntaxPoint{Line: bp.Line, Col: bp.Col}
	}

	lc := s.lineConvs[bp.Line]
	if len(lc.deltas) == 0 {
		return coords.SyntaxPoint{Line: bp.Line, Col: bp.Col}
	}

	// Clamp position out of hidden ranges
	col := clampCol(bp.Col, lc.hidden)

	// Binary search for the last delta entry with BufferOffset <= col
	idx := sort.Search(len(lc.deltas), func(i int) bool {
		return lc.deltas[i].BufferOffset > col
	}) - 1

	delta := 0
	if idx >= 0 {
		delta = lc.deltas[idx].Delta
	}

	syntaxCol := col - delta
	if syntaxCol < 0 {
		syntaxCol = 0
	}

	return coords.SyntaxPoint{Line: bp.Line, Col: syntaxCol}
}

// SyntaxToBuffer converts a syntax-space point back to buffer-space.
func (s SyntaxSnapshot) SyntaxToBuffer(sp coords.SyntaxPoint) coords.BufferPoint {
	if sp.Line < 0 || sp.Line >= len(s.lineConvs) {
		return coords.BufferPoint{Line: sp.Line, Col: sp.Col}
	}

	lc := s.lineConvs[sp.Line]
	if len(lc.deltas) == 0 {
		return coords.BufferPoint{Line: sp.Line, Col: sp.Col}
	}

	// Reverse mapping: find the delta region where sp.Col falls.
	// For each delta entry at BufferOffset B with Delta D:
	//   syntax col at B = B - D
	// We need the last entry i where (B_i - D_i) <= sp.Col
	idx := -1
	for i, d := range lc.deltas {
		syntaxAtEntry := d.BufferOffset - d.Delta
		if syntaxAtEntry <= sp.Col {
			idx = i
		} else {
			break
		}
	}

	delta := 0
	if idx >= 0 {
		delta = lc.deltas[idx].Delta
	}

	return coords.BufferPoint{Line: sp.Line, Col: sp.Col + delta}
}

// clampCol adjusts a buffer column that falls inside a hidden range.
func clampCol(col int, hidden []hiddenRange) int {
	for _, h := range hidden {
		if col >= h.start && col < h.end {
			return h.clampTo
		}
		if h.start > col {
			break // sorted, no more matches
		}
	}
	return col
}

// NewSyntaxSnapshotFromLines creates a SyntaxSnapshot with identity coordinate
// mapping (no hidden ranges, no deltas). Used for testing wrap behavior in
// isolation without markdown token folding.
func NewSyntaxSnapshotFromLines(lines []SyntaxLine) SyntaxSnapshot {
	convs := make([]lineConversion, len(lines))
	// All lineConversion entries default to empty deltas/hidden = identity mapping
	return SyntaxSnapshot{
		Lines:     lines,
		lineConvs: convs,
	}
}
